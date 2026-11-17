//! Sampled audits of landings whose change landed after its review: `builds
//! review landed` samples them as `builds land` records each. An audit asks again,
//! two other ways, and anything it raises that the landing's review did not
//! becomes a candidate miss for the coordinator to settle: one reviewer per
//! rule over every rule that covers a changed path, and one review with the
//! rules shuffled into a different grouping. A second model checks each
//! candidate miss against its rule's text (`model_check.rs`), and the audit
//! settles it with the model's answer: confirmed, or dropped with the model's
//! reason. The record keeps each group's audited miss rate, which `builds
//! review rates` prints.

use std::path::Path;

use rusqlite::Connection;

use crate::fail::Result;
use crate::lists::Stamp;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::Store;
use crate::review::groups::{Group, Groups};
use crate::review::model_check;
use crate::review::record::{self, Call, Finding, Kind, NewRun};
use crate::review::render;
use crate::review::rules::Rule;
use crate::review::run::{self, Diff, Material};

/// The share of landings audited when nothing says to audit more.
pub const BASE_CHANCE: f64 = 0.05;
/// The share for a landing that ran a group whose audited misses rose.
pub const RISEN_CHANCE: f64 = 0.25;
/// Every landing is audited until this many have been reviewed under a grouping.
pub const FULL_FOR: i64 = 20;
/// The audits a group's recent miss rate is taken over.
const RECENT: usize = 10;

/// The share of landings to audit: all of them for the first `FULL_FOR`
/// under a grouping, more for a landing that ran a group whose misses rose.
pub fn chance(landings_under_grouping: i64, risen: bool) -> f64 {
    if landings_under_grouping <= FULL_FOR {
        1.0
    } else if risen {
        RISEN_CHANCE
    } else {
        BASE_CHANCE
    }
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

fn group_of<'a>(groups: &'a Groups, slug: &str) -> Option<&'a Group> {
    groups
        .groups
        .iter()
        .find(|g| g.rules.iter().any(|r| r == slug))
}

fn synthetic(name: String, model: &str, question: &str, rules: &[&Rule]) -> Group {
    Group {
        name,
        dir: String::new(),
        question: question.to_string(),
        model: model.to_string(),
        rules: rules.iter().map(|r| r.slug.clone()).collect(),
        instructions: None,
        project_facts: false,
        enumerate: false,
        sites: None,
        commit_text: false,
        passes: 1,
    }
}

/// One group per rule, each asked in the model and the words of the rule's own group.
pub fn per_rule_groups(groups: &Groups, matched: &[&Rule]) -> Vec<Group> {
    matched
        .iter()
        .map(|rule| {
            let (model, question) = group_of(groups, &rule.slug)
                .map(|g| (g.model.as_str(), g.question.clone()))
                .unwrap_or(("sonnet", String::new()));
            synthetic(format!("audit {}", rule.slug), model, &question, &[rule])
        })
        .collect()
}

/// The rules in a random order, cut into groups of <size>.
pub fn shuffled_groups(matched: &[&Rule], size: usize, rng: &mut Rng) -> Vec<Group> {
    let mut order: Vec<&Rule> = matched.to_vec();
    for i in (1..order.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
        .chunks(size.max(1))
        .enumerate()
        .map(|(n, chunk)| {
            synthetic(
                format!("shuffle {}", n + 1),
                "sonnet",
                "Does the change follow each of these rules?",
                chunk,
            )
        })
        .collect()
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
        let calls: Vec<Call> = record::calls_of_landing(db, landing)?;
        let missed: Vec<String> = candidate_misses(&findings)
            .into_iter()
            .filter(|f| f.settled_as("confirmed"))
            .filter_map(|f| group_of_rule(&f.rule))
            .collect();
        let mut ran: Vec<&str> = Vec::new();
        for call in calls.iter().filter(|c| c.kind == "review") {
            if !ran.contains(&call.group.as_str()) {
                ran.push(&call.group);
            }
        }
        for group in ran {
            let miss = missed.iter().any(|g| g == group);
            match outcomes.iter_mut().find(|(g, _)| g == group) {
                Some((_, list)) => list.push(miss),
                None => outcomes.push((group.to_string(), vec![miss])),
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
    let Some(run) = record::landing(db, landing)?.landed_run else {
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
    let Some(landed) = landed_run(db, landing)? else {
        return Ok(false);
    };
    let under = record::landings_under(db, &landed.grouping)?;
    let ran: Vec<String> = record::calls_of_landing(db, landing)?
        .into_iter()
        .filter(|c| c.kind == "review")
        .map(|c| c.group)
        .collect();
    let risen = rates(db, group_of_rule)?
        .iter()
        .any(|rate| rate.risen() && ran.contains(&rate.group));
    Ok(roll < chance(under, risen))
}

pub struct Inputs<'a> {
    pub store: &'a crate::lists::store::Store,
    pub project: &'a str,
    /// A checkout of the repo that holds the landing's commits.
    pub repo_dir: &'a Path,
    pub landing: i64,
    pub root: &'a Path,
    pub reviewer: &'a Path,
    pub stamp: &'a crate::lists::Stamp,
    pub seed: u64,
}

/// Audits a landing whose change landed and records both audits, over the
/// commits of the run it landed with. Returns what to print: the candidate
/// misses and how to settle them.
pub fn audit(inputs: &Inputs) -> Result<String> {
    let conn = inputs.store.open_existing()?;
    let key = record::landing(&conn, inputs.landing)?.key;
    let Some(last) = landed_run(&conn, inputs.landing)? else {
        crate::fail!(
            "landing {} ({key}) has not landed, so there is nothing to audit",
            inputs.landing
        );
    };
    let diff = run::diff_between(inputs.repo_dir, &last.base, &last.head)?;
    let material = Material::load(inputs.root, inputs.project)?;
    let matched: Vec<&Rule> = material
        .rules
        .iter()
        .filter(|r| r.reviewable && r.matches(&diff.changed))
        .collect();
    if matched.is_empty() {
        return Ok("no rule covers the changed paths: nothing to audit\n".to_string());
    }
    let reviewed_groups = material
        .groups
        .pick(&material.rules, &diff.changed)
        .len()
        .max(1);
    let size = matched.len().div_ceil(reviewed_groups);
    let per_rule = per_rule_groups(&material.groups, &matched);
    let mut rng = Rng(inputs.seed | 1);
    let shuffled = shuffled_groups(&matched, size, &mut rng);
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
        run::ask_groups(&material, inputs.reviewer, &picked, &diff, None, &[]).calls
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
                        &NewRun {
                            landing: Some(inputs.landing),
                            kind: Kind::Audit,
                            label: Some(label),
                            at: stamp.at,
                            base: &last.base,
                            head: &last.head,
                            since: None,
                            grouping: &material.grouping,
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
    let mut text = format!(
        "audit of landing {} ({key}): {} candidate misses the review did not raise\n",
        inputs.landing,
        fresh.len()
    );
    let (settled, _) = settle_misses(
        inputs.store,
        stamp,
        &material,
        inputs.reviewer,
        model_check::MODEL,
        &diff,
        &fresh,
    )?;
    text.push_str(&settled);
    Ok(text)
}

/// Asks the model check about each of <misses>, which an audit raised over
/// <diff>, and settles each one the model answers: confirmed, or dropped with
/// the model's reason. A miss whose check gave no answer stays unsettled.
/// Returns what to print, and whether a check gave no answer.
pub fn settle_misses(
    store: &Store,
    stamp: &Stamp,
    material: &Material,
    program: &Path,
    model: &str,
    diff: &Diff,
    misses: &[&Finding],
) -> Result<(String, bool)> {
    let checked = crate::checkout::parallel_map(misses, |miss| {
        model_check::check(program, model, material, miss, diff)
    });
    let mut text = String::new();
    let mut failed = false;
    let mut settles = Vec::new();
    for (miss, checked) in misses.iter().zip(&checked) {
        text.push_str(&render::finding_text(miss));
        match checked.settle_why() {
            Some((how, why)) => {
                text.push_str(&format!("    settled {how}: {why}\n"));
                settles.push((miss.id, how, why));
            }
            None => {
                failed = true;
                text.push_str(&format!(
                    "    the check gave no answer ({}); settle it: builds review settle {} confirmed|dropped --why TEXT\n",
                    checked.verdict.as_ref().err().map(String::as_str).unwrap_or(""),
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
            "A confirmed miss becomes a case (builds review case propose) and a fix item.\n",
        );
    }
    Ok((text, failed))
}

/// What `builds review misses --check` reads.
pub struct OpenMisses<'a> {
    pub store: &'a Store,
    pub project: &'a str,
    /// A checkout of the repo that holds the audited landings' commits.
    pub repo_dir: &'a Path,
    pub root: &'a Path,
    pub reviewer: &'a Path,
    pub model: &'a str,
    pub stamp: &'a Stamp,
}

/// Settles every candidate miss nobody has settled, as an audit settles the
/// misses it raises (`settle_misses`), over the net change of the run each
/// audited landing landed with. Returns what to print, and whether a diff
/// could not be read or a check gave no answer.
pub fn check_open_misses(inputs: &OpenMisses) -> Result<(String, bool)> {
    let conn = inputs.store.open_existing()?;
    let material = Material::load(inputs.root, inputs.project)?;
    let mut text = String::new();
    let mut failed = false;
    let mut count = 0;
    for landing in record::audited_landings(&conn)? {
        let findings = record::findings_of_landing(&conn, landing)?;
        let open: Vec<&Finding> = candidate_misses(&findings)
            .into_iter()
            .filter(|f| f.settled.is_none())
            .collect();
        if open.is_empty() {
            continue;
        }
        count += open.len();
        text.push_str(&format!("landing {landing}:\n"));
        let Some(last) = landed_run(&conn, landing)? else {
            failed = true;
            text.push_str("  has not landed, so builds has no net change to read\n");
            continue;
        };
        let diff = match crate::review::rerun::net_change(inputs.repo_dir, &last)
            .and_then(|(base, head)| run::diff_between(inputs.repo_dir, &base, &head))
        {
            Ok(diff) => diff,
            Err(why) => {
                failed = true;
                text.push_str(&format!("  not checked: {}\n", why.text()));
                continue;
            }
        };
        let (settled, check_failed) = settle_misses(
            inputs.store,
            inputs.stamp,
            &material,
            inputs.reviewer,
            inputs.model,
            &diff,
            &open,
        )?;
        failed |= check_failed;
        text.push_str(&settled);
    }
    text.push_str(&format!(
        "{} unsettled before the check\n",
        crate::text::counted(count, "candidate miss", "candidate misses")
    ));
    Ok((text, failed))
}

/// `builds review misses --check --repo-dir DIR [--model M]`: settles every
/// candidate miss nobody has settled with the model check, and exits 1 when a
/// diff could not be read or a check gave no answer.
pub(super) fn check_misses_command(tool: &crate::tool::Tool, args: &Args) -> Result<()> {
    let model = args.value("model")?.unwrap_or(model_check::MODEL);
    if !["opus", "sonnet", "haiku"].contains(&model) {
        crate::fail!("--model is opus, sonnet or haiku, not {model}");
    }
    let store = Store::resolve(&tool.places, args, &current_dir()?)?;
    let root = super::rules_root(args)?;
    let reviewer = std::path::PathBuf::from(args.value("reviewer")?.unwrap_or("claude"));
    let repo_dir = std::path::PathBuf::from(args.require("repo-dir")?);
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
        repo_dir: &repo_dir,
        root: &root,
        reviewer: &reviewer,
        model,
        stamp: &stamp,
    })?;
    print!("{text}");
    if failed {
        return Err(crate::fail::Fail::Exit(1));
    }
    Ok(())
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
