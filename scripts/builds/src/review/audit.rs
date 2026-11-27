//! Sampled audits of landings whose change landed after its review: `builds
//! review landed` samples them as `builds land` records each. An audit asks again,
//! two other ways, and anything it raises that the landing's review did not
//! becomes a candidate miss for the coordinator to settle. Both ways ask only
//! the rules of the groups the landing's review ran that cover a changed
//! path, each on the model of its own group: one reviewer per rule, and one
//! review with the rules shuffled into a different grouping. A second model checks each
//! candidate miss against its rule's text (`model_check.rs`), and the audit
//! settles it with the model's answer: confirmed, or dropped with the model's
//! reason. The record keeps each group's audited miss rate, which `builds
//! review rates` prints.

use super::provider::Selection;

use std::path::Path;

use rusqlite::Connection;

use crate::args::Args;
use crate::fail::Result;
use crate::lists::Stamp;
use crate::lists::store::Store;
use crate::review::check;
use crate::review::files;
use crate::review::groups::Group;
use crate::review::model_check;
use crate::review::prompt;
use crate::review::record::{self, Finding, Kind, NewRun};
use crate::review::render;
use crate::review::rules::Rule;
use crate::review::run::{self, Diff, Material};
use crate::review::texts::Texts;

/// The share of landings audited when nothing says to audit more.
pub const BASE_CHANCE: f64 = 0.05;
/// The share for a landing that ran a group whose audited misses rose.
pub const RISEN_CHANCE: f64 = 0.25;
/// The audits a group's recent miss rate is taken over.
const RECENT: usize = 10;

/// The share of landings to audit: more for a landing that ran a group whose
/// misses rose.
pub fn chance(risen: bool) -> f64 {
    if risen { RISEN_CHANCE } else { BASE_CHANCE }
}

/// A small deterministic generator, so a shuffle can be repeated from its seed.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn synthetic(name: String, model: &Selection, question: &str, rules: &[&Rule]) -> Group {
    Group {
        name,
        dir: String::new(),
        question: question.to_string(),
        model: model.clone(),
        rules: rules.iter().map(|r| r.slug.clone()).collect(),
        instructions: None,
        project_facts: false,
        enumerate: false,
        sites: None,
        commit_text: false,
        skips_comments: false,
        copy: false,
        passes: 1,
    }
}

/// One group per rule of <audited>, each asked in the model and the words of
/// the rule's own group.
pub fn per_rule_groups(audited: &[(&Group, Vec<&Rule>)]) -> Vec<Group> {
    audited
        .iter()
        .flat_map(|(group, rules)| {
            rules.iter().map(|rule| {
                synthetic(
                    format!("audit {}", rule.slug),
                    &group.model,
                    &group.question,
                    &[*rule],
                )
            })
        })
        .collect()
}

/// The rules of <audited> in a random order, cut into groups of <size>. A
/// shuffled group holds only rules whose own groups run one model, and runs
/// that model, so the shuffle changes the grouping and nothing else.
pub fn shuffled_groups(audited: &[(&Group, Vec<&Rule>)], size: usize, rng: &mut Rng) -> Vec<Group> {
    let mut models: Vec<&Selection> = Vec::new();
    for (group, _) in audited {
        if !models.contains(&&group.model) {
            models.push(&group.model);
        }
    }
    let mut shuffled = Vec::new();
    for model in models {
        let mut order: Vec<&Rule> = audited
            .iter()
            .filter(|(group, _)| &group.model == model)
            .flat_map(|(_, rules)| rules.iter().copied())
            .collect();
        for i in (1..order.len()).rev() {
            let j = (rng.next() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        for chunk in order.chunks(size.max(1)) {
            shuffled.push(synthetic(
                format!("shuffle {}", shuffled.len() + 1),
                model,
                "Does the change follow each of these rules?",
                chunk,
            ));
        }
    }
    shuffled
}

/// The names of the groups the review runs of <landing> asked, each once, in
/// the order first asked.
fn groups_ran(db: &Connection, landing: i64) -> Result<Vec<String>> {
    let mut ran: Vec<String> = Vec::new();
    for call in record::calls_of_landing(db, landing)? {
        if call.kind == "review" && !ran.contains(&call.group) {
            ran.push(call.group);
        }
    }
    Ok(ran)
}

/// The findings of audits that the landing's review did not raise: an audit
/// finding builds accepted, in a rule and file no accepted review finding of
/// the landing has.
pub fn candidate_misses(findings: &[Finding]) -> Vec<&Finding> {
    let same = |a: &Finding, b: &Finding| a.rule == b.rule && a.file == b.file;
    findings
        .iter()
        .filter(|f| f.kind == "audit" && f.counts())
        .filter(|f| {
            !findings
                .iter()
                .any(|r| r.kind == "review" && r.counts() && same(f, r))
        })
        .collect()
}

/// One group's audited miss rate.
#[derive(Clone, Debug, PartialEq)]
pub struct Rate {
    pub group: String,
    pub audits: usize,
    pub misses: usize,
    /// Misses among the latest audits, and how many those were.
    pub recent_misses: usize,
    pub recent_audits: usize,
}

impl Rate {
    /// Whether the group's recent audits missed more often than its earlier ones.
    pub fn risen(&self) -> bool {
        let earlier_audits = self.audits - self.recent_audits;
        if self.recent_misses == 0 || earlier_audits == 0 {
            return false;
        }
        let recent = self.recent_misses as f64 / self.recent_audits as f64;
        let earlier = (self.misses - self.recent_misses) as f64 / earlier_audits as f64;
        recent > earlier
    }
}

/// Each group's audited miss rate: over the audited landings, oldest first, a
/// group that ran in the landing's review missed when a confirmed candidate
/// miss is in one of its rules (by <group_of_rule>).
pub fn rates(db: &Connection, group_of_rule: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Rate>> {
    let mut outcomes: Vec<(String, Vec<bool>)> = Vec::new();
    for landing in record::audited_landings(db)? {
        let findings = record::findings_of_landing(db, landing)?;
        let missed: Vec<String> = candidate_misses(&findings)
            .into_iter()
            .filter(|f| f.settled_as("confirmed"))
            .filter_map(|f| group_of_rule(&f.rule))
            .collect();
        for group in groups_ran(db, landing)? {
            let miss = missed.contains(&group);
            match outcomes.iter_mut().find(|(g, _)| *g == group) {
                Some((_, list)) => list.push(miss),
                None => outcomes.push((group, vec![miss])),
            }
        }
    }
    outcomes.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(outcomes
        .into_iter()
        .map(|(group, list)| {
            let recent: &[bool] = &list[list.len().saturating_sub(RECENT)..];
            Rate {
                group,
                audits: list.len(),
                misses: list.iter().filter(|m| **m).count(),
                recent_misses: recent.iter().filter(|m| **m).count(),
                recent_audits: recent.len(),
            }
        })
        .collect())
}

/// The run <landing> landed with; None while it has not landed.
fn landed_run(db: &Connection, landing: i64) -> Result<Option<record::RunRow>> {
    let Some(run) = record::landing(db, landing)?.landed_run() else {
        return Ok(None);
    };
    Ok(record::runs_of_landing(db, landing)?
        .into_iter()
        .find(|r| r.id == run))
}

/// Whether <landing>, a landed change, is audited when the dice read <roll>
/// (from 0 up to, not including, 1).
pub fn sampled(
    db: &Connection,
    landing: i64,
    group_of_rule: &dyn Fn(&str) -> Option<String>,
    roll: f64,
) -> Result<bool> {
    if landed_run(db, landing)?.is_none() {
        return Ok(false);
    }
    let ran = groups_ran(db, landing)?;
    let risen = rates(db, group_of_rule)?
        .iter()
        .any(|rate| rate.risen() && ran.contains(&rate.group));
    Ok(roll < chance(risen))
}

pub struct Inputs<'a> {
    pub store: &'a crate::lists::store::Store,
    pub project: &'a str,
    /// A checkout of the repo that holds the landing's commits.
    pub repo_dir: &'a Path,
    pub landing: i64,
    pub root: &'a Path,
    pub reviewer: &'a crate::review::Program,
    pub stamp: &'a crate::lists::Stamp,
    pub seed: u64,
}

/// Audits a landing whose change landed and records both audits, over the
/// commits of the run it landed with, asking the rules of the groups its
/// review ran. Returns what to print: the candidate misses and how to settle
/// them.
pub fn audit(inputs: &Inputs) -> Result<String> {
    let conn = inputs.store.open_existing()?;
    let row = record::landing(&conn, inputs.landing)?;
    let key = row.key;
    // The design files of the landing's repo.
    let design = crate::lists::design_paths::Globs::read(&conn, &row.repo)?
        .with_capture_folders(inputs.repo_dir)?;
    let Some(last) = landed_run(&conn, inputs.landing)? else {
        crate::fail!(
            "landing {} ({key}) has not landed, so there is nothing to audit",
            inputs.landing
        );
    };
    let material = Material::load(inputs.root, inputs.project)?;
    let unread = material.unread(&row.repo);
    let diff = run::diff_between(inputs.repo_dir, &last.base, &last.head, &unread)?.diff;
    let ran = groups_ran(&conn, inputs.landing)?;
    let reviewed = material.groups.named(&material.rules, &ran);
    let mut audited: Vec<(&Group, Vec<&Rule>)> = Vec::new();
    let mut text = String::new();
    for name in &ran {
        let Some((group, members)) = reviewed.iter().find(|(group, _)| &group.name == name) else {
            text.push_str(&format!(
                "not audited: group {name} ran on the landing's review, and the rules checkout has no group of that name that `builds review` asks\n"
            ));
            continue;
        };
        let covering: Vec<&Rule> = members
            .iter()
            .copied()
            .filter(|rule| rule.matches(&diff.changed, &design))
            .collect();
        if covering.is_empty() {
            text.push_str(&format!(
                "not audited: group {name} has no rule in the rules checkout that covers the changed paths\n"
            ));
            continue;
        }
        audited.push((*group, covering));
    }
    if audited.is_empty() {
        text.push_str(&format!(
            "audit of landing {} ({key}): no group the landing's review ran is left to audit\n",
            inputs.landing
        ));
        return Ok(text);
    }
    let matched: Vec<&Rule> = audited
        .iter()
        .flat_map(|(_, rules)| rules.iter().copied())
        .collect();
    let size = matched.len().div_ceil(audited.len());
    let per_rule = per_rule_groups(&audited);
    let mut rng = Rng(inputs.seed | 1);
    let shuffled = shuffled_groups(&audited, size, &mut rng);
    let ask = |groups: &[Group]| {
        let picked: Vec<(&Group, Vec<&Rule>)> = groups
            .iter()
            .map(|g| {
                let members = g
                    .rules
                    .iter()
                    .filter_map(|slug| matched.iter().find(|r| &r.slug == slug).copied())
                    .collect();
                (g, members)
            })
            .collect();
        let scope = run::Scope {
            material: &material,
            files: &design,
            unread: &unread,
        };
        run::ask_groups(
            scope,
            inputs.reviewer,
            run::ReviewKind::Check,
            &picked,
            &diff,
            None,
            &[],
        )
        .calls
    };
    let calls_rules = ask(&per_rule);
    let calls_shuffled = ask(&shuffled);
    let stamp = inputs.stamp;
    let (runs, findings) =
        inputs
            .store
            .write(stamp, &format!("review audit {}", inputs.landing), |db| {
                let mut runs = Vec::new();
                for (label, calls) in [
                    ("one reviewer per rule", &calls_rules),
                    ("rules shuffled into other groups", &calls_shuffled),
                ] {
                    let (id, _) = record::add_run(
                        db,
                        &inputs.store.texts,
                        &NewRun {
                            landing: Some(inputs.landing),
                            kind: Kind::Audit,
                            label: Some(label),
                            at: stamp.at,
                            base: &last.base,
                            head: &last.head,
                            since: None,
                            rules_version: &run::rules_version(inputs.root),
                        },
                        calls,
                    )?;
                    runs.push(id);
                }
                Ok((runs, record::findings_of_landing(db, inputs.landing)?))
            })?;
    let fresh: Vec<&Finding> = candidate_misses(&findings)
        .into_iter()
        .filter(|f| runs.contains(&f.run_id))
        .collect();
    text.push_str(&format!(
        "audit of landing {} ({key}): {} candidate misses the review did not raise\n",
        inputs.landing,
        fresh.len()
    ));
    let (settled, _) = settle_misses(
        inputs.store,
        stamp,
        &material,
        inputs.reviewer,
        &model_check::MODEL,
        &fresh.iter().map(|miss| (*miss, &diff)).collect::<Vec<_>>(),
    )?;
    text.push_str(&settled);
    Ok(text)
}

/// Asks the model check about each of <misses>, each with the diff an audit
/// raised it over, and settles each one the model answers: confirmed, or dropped with
/// the model's reason. A miss whose check gave no answer stays unsettled.
/// Returns what to print, and whether a check gave no answer.
pub fn settle_misses(
    store: &Store,
    stamp: &Stamp,
    material: &Material,
    program: &crate::review::Program,
    model: &Selection,
    misses: &[(&Finding, &Diff)],
) -> Result<(String, bool)> {
    let checked = check::reviewers_map(misses, |(miss, diff)| {
        model_check::check(program, model, material, miss, diff)
    });
    let mut text = String::new();
    let mut failed = false;
    let mut settles = Vec::new();
    for ((miss, _), checked) in misses.iter().zip(&checked) {
        text.push_str(&render::finding_text(miss));
        match checked
            .as_ref()
            .map_err(String::as_str)
            .and_then(model_check::Checked::settle_why)
        {
            Ok((how, why)) => {
                text.push_str(&format!("    settled {how}: {why}\n"));
                settles.push((miss.id, how, why));
            }
            Err(why) => {
                failed = true;
                text.push_str(&format!(
                    "    The check gave no answer ({why}). Run builds review misses --check to retry finding {}.\n",
                    miss.id
                ));
            }
        }
    }
    if !settles.is_empty() {
        store.write(stamp, "review settle misses", |db| {
            for (id, how, why) in &settles {
                record::settle(db, *id, how, why, stamp.at, &stamp.by)?;
            }
            Ok(())
        })?;
    }
    if settles.iter().any(|(_, how, _)| *how == "confirmed") {
        text.push_str(
            "For each confirmed miss, the coordinator adds a fix item, and builds review case propose writes its case.\n",
        );
    }
    Ok((text, failed))
}

/// What `builds review misses --check` reads.
pub struct OpenMisses<'a> {
    pub store: &'a Store,
    pub project: &'a str,
    pub root: &'a Path,
    pub reviewer: &'a crate::review::Program,
    pub model: &'a Selection,
    pub stamp: &'a Stamp,
}

/// The candidate misses nobody has settled, each with its landing. A miss is
/// a candidate against the review findings of its own landing.
pub fn open_misses(db: &Connection) -> Result<Vec<(i64, Finding)>> {
    let mut open = Vec::new();
    for landing in record::audited_landings(db)? {
        let findings = record::findings_of_landing(db, landing)?;
        open.extend(
            candidate_misses(&findings)
                .into_iter()
                .filter(|f| f.settled.is_none())
                .map(|f| (landing, f.clone())),
        );
    }
    Ok(open)
}

/// The diff the audit reviewer that raised <miss> read, from the message the
/// store keeps; the commits it read may be gone from every repo.
pub(super) fn diff_read(
    db: &Connection,
    texts: &Texts,
    miss: &Finding,
) -> std::result::Result<Diff, String> {
    let messages =
        record::messages_of_call(db, texts, miss.run_id, &miss.group).map_err(|why| why.text())?;
    let text = messages
        .first()
        .and_then(|message| prompt::diff_of_message(message))
        .ok_or("the store keeps no diff of the call that raised it")?
        .to_string();
    let patch = files::Patch::parse(text).map_err(crate::fail::Fail::text)?;
    Ok(Diff::from_patch(patch, String::new(), None))
}

/// Settles every candidate miss nobody has settled, as an audit settles the
/// misses it raises (`settle_misses`), over the diff the audit reviewer that
/// raised it read. Returns what to print, and whether builds found no stored
/// diff for a miss or a check gave no answer.
pub fn check_open_misses(inputs: &OpenMisses) -> Result<(String, bool)> {
    let conn = inputs.store.open_existing()?;
    let material = Material::load(inputs.root, inputs.project)?;
    let mut text = String::new();
    let mut failed = false;
    let open = open_misses(&conn)?;
    let mut asked: Vec<(&Finding, Diff)> = Vec::new();
    for (_, miss) in &open {
        match diff_read(&conn, &inputs.store.texts, miss) {
            Ok(diff) => asked.push((miss, diff)),
            Err(why) => {
                failed = true;
                text.push_str(&format!(
                    "builds did not check finding {}: {why}\n",
                    miss.id
                ));
            }
        }
    }
    drop(conn);
    let pairs: Vec<(&Finding, &Diff)> = asked.iter().map(|(miss, diff)| (*miss, diff)).collect();
    let (settled, check_failed) = settle_misses(
        inputs.store,
        inputs.stamp,
        &material,
        inputs.reviewer,
        inputs.model,
        &pairs,
    )?;
    failed |= check_failed;
    text.push_str(&settled);
    text.push_str(&format!(
        "the check found {} that nobody had settled\n",
        crate::text::counted(open.len(), "candidate miss", "candidate misses")
    ));
    Ok((text, failed))
}

/// `builds review misses --check [--model M]`: settles every candidate miss
/// nobody has settled with the model check, and exits 1 when builds found no
/// stored diff for a miss or a check gave no answer.
pub(super) fn check_misses_command(tool: &crate::tool::Tool, args: &Args) -> Result<()> {
    let model = model_check::selection(args.value("model")?);
    let store = Store::resolve(&tool.places, args, &tool.dir)?;
    let root = super::rules_root(args)?;
    let reviewer = tool.reviewer.program_for(args.value("reviewer")?);
    let stamp = Stamp {
        at: tool.now,
        by: args
            .value("by")?
            .unwrap_or("builds model check")
            .to_string(),
    };
    let (text, failed) = check_open_misses(&OpenMisses {
        store: &store,
        project: &store.project,
        root: &root,
        reviewer: &reviewer,
        model: &model,
        stamp: &stamp,
    })?;
    crate::output::answer(text);
    if failed {
        return Err(crate::fail::Fail::Exit(1));
    }
    Ok(())
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
