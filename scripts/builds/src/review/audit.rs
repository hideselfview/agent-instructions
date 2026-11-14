//! Sampled audits of landings that passed their review. An audit asks again,
//! two other ways, and anything it raises that the landing's review did not
//! becomes a candidate miss for the coordinator to settle: one reviewer per
//! rule over every rule that covers a changed path, and one review with the
//! rules shuffled into a different grouping. The record keeps each group's
//! audited miss rate, which the optimizer reads.

use std::path::Path;

use rusqlite::Connection;

use crate::fail::Result;
use crate::review::groups::{Group, Groups};
use crate::review::record::{self, Call, Finding, Kind, NewRun};
use crate::review::render;
use crate::review::rules::Rule;
use crate::review::run::{self, Material};

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

fn synthetic(name: String, model: &str, question: &str, rules: &[&Rule], groups: &Groups) -> Group {
    let mut principles: Vec<String> = Vec::new();
    for rule in rules {
        if let Some(original) = group_of(groups, &rule.slug) {
            for p in &original.principles {
                if !principles.contains(p) {
                    principles.push(p.clone());
                }
            }
        }
    }
    Group {
        name,
        question: question.to_string(),
        model: model.to_string(),
        rules: rules.iter().map(|r| r.slug.clone()).collect(),
        principles,
        instructions: None,
        project_facts: false,
        enumerate: false,
        sites: None,
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
            synthetic(
                format!("audit {}", rule.slug),
                model,
                &question,
                &[rule],
                groups,
            )
        })
        .collect()
}

/// The rules in a random order, cut into groups of <size>.
pub fn shuffled_groups(
    groups: &Groups,
    matched: &[&Rule],
    size: usize,
    rng: &mut Rng,
) -> Vec<Group> {
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
                groups,
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
            .filter(|f| matches!(&f.settled, Some((how, _)) if how == "confirmed"))
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

/// Whether <landing>, a passed review, is audited when the dice read <roll>
/// (from 0 up to, not including, 1).
pub fn sampled(
    db: &Connection,
    landing: i64,
    group_of_rule: &dyn Fn(&str) -> Option<String>,
    roll: f64,
) -> Result<bool> {
    let runs = record::runs_of_landing(db, landing)?;
    let Some(last) = runs.iter().rev().find(|r| r.kind == "review") else {
        return Ok(false);
    };
    let under = record::landings_under(db, &last.grouping)?;
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

/// Audits a landing that passed its review and records both audits. Returns
/// what to print: the candidate misses and how to settle them.
pub fn audit(inputs: &Inputs) -> Result<String> {
    let conn = crate::db::open_reader(&inputs.store.database())?;
    let Some(conn) = conn else {
        crate::fail!("there is no store for project {}", inputs.project);
    };
    let (key, _, closed) = record::landing(&conn, inputs.landing)?;
    if !closed {
        crate::fail!(
            "landing {} ({key}) has no passed review to audit",
            inputs.landing
        );
    }
    let last = record::runs_of_landing(&conn, inputs.landing)?
        .into_iter()
        .rev()
        .find(|r| r.kind == "review")
        .expect("a passed landing has a review run");
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
    let shuffled = shuffled_groups(&material.groups, &matched, size, &mut rng);
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
        run::ask_groups(&material, inputs.reviewer, &picked, &diff, None, &|_| {
            Vec::new()
        })
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
                            grouping: &material.grouping,
                        },
                        calls,
                    )?;
                    runs.push(id);
                }
                Ok((runs, record::findings_of_landing(db, inputs.landing)?))
            })?;
    let mut text = String::new();
    let fresh: Vec<&Finding> = candidate_misses(&findings)
        .into_iter()
        .filter(|f| runs.contains(&f.run_id))
        .collect();
    text.push_str(&format!(
        "audit of landing {} ({key}): {} candidate misses the review did not raise\n",
        inputs.landing,
        fresh.len()
    ));
    for finding in &fresh {
        text.push_str(&render::finding_text(finding));
    }
    if !fresh.is_empty() {
        text.push_str(
            "Settle each: builds review settle <finding> confirmed|dropped --why TEXT. A confirmed miss becomes a case (builds review case add) and a fix item.\n",
        );
    }
    Ok(text)
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
