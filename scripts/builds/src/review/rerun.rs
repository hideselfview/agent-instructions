//! `builds review replay --landing N`: asks again over each diff that landing
//! N's reviews read, under the rules checkout, model and groups given, records
//! each replay in the store, and scores it against the findings the coordinator
//! settled, next to what the recorded reviews did (`score.rs`). Replayed
//! reviewers get no brief, since the store does not keep it apart from the
//! prompt.
//!
//! A run's diff is read from its base and head when the repo still has both
//! commits. Otherwise it is read from the commits of the repo's history whose
//! patch ids are the run's, one after another, so a landed change whose branch
//! is gone still replays.

use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::lists::Stamp;
use crate::lists::store::Store;
use crate::review::audit::candidate_misses;
use crate::review::commits::{self, Commit};
use crate::review::prompt;
use crate::review::record::{self, Call, Finding, Kind, NewRun, Verdict};
use crate::review::render;
use crate::review::run::{self, Diff, Material};
use crate::review::score::{self, Scores};

/// How many of the repo's newest commits are searched for a run's patch ids.
const SEARCHED: usize = 1000;

pub struct Inputs<'a> {
    pub store: &'a Store,
    pub project: &'a str,
    /// A checkout of the repo that holds the landings' commits.
    pub repo_dir: &'a Path,
    pub landings: &'a [i64],
    pub root: &'a Path,
    pub reviewer: &'a Path,
    /// The model every group is asked in; each group's own when None.
    pub model: Option<&'a str>,
    /// The groups to ask; every group that covers a changed path when empty.
    pub groups: &'a [&'a str],
    pub stamp: &'a Stamp,
}

/// What a replay of landings found.
pub struct Replayed {
    pub text: String,
    /// Whether a diff could not be read or a call gave no answer.
    pub failed: bool,
}

/// Replays every diff the reviews of <inputs>' landings read, records each
/// replay, and returns what to print: each diff's new findings, then the
/// replay's counts next to the recorded reviews'.
pub fn replay(inputs: &Inputs) -> Result<Replayed> {
    let conn = inputs.store.open_existing()?;
    let mut material = Material::load(inputs.root, inputs.project)?;
    if let Some(model) = inputs.model {
        material = material.with_model(model);
    }
    for name in inputs.groups {
        if !material.groups.groups.iter().any(|g| g.name == *name) {
            fail!("there is no group {name}");
        }
    }
    let rules_version = run::rules_version(inputs.root);
    let mut text = String::new();
    let mut failed = false;
    let mut in_use = Scores::default();
    let mut replayed = Scores::default();
    for &landing in inputs.landings {
        let key = record::landing(&conn, landing)?.key;
        let runs = record::runs_of_landing(&conn, landing)?;
        let calls = record::calls_of_landing(&conn, landing)?;
        let verdicts = record::verdicts_of_landing(&conn, landing)?;
        let findings = record::findings_of_landing(&conn, landing)?;
        let misses: Vec<&Finding> = candidate_misses(&findings)
            .into_iter()
            .filter(|f| f.settled.is_some())
            .collect();
        let mut diffs: Vec<(&str, &str)> = Vec::new();
        for run in runs.iter().filter(|r| r.kind == "review") {
            let diff = (run.base.as_str(), run.head.as_str());
            if !diffs.contains(&diff) {
                diffs.push(diff);
            }
        }
        if diffs.is_empty() {
            text.push_str(&format!(
                "landing {landing} ({key}) has no review to replay\n"
            ));
        }
        for (base, head) in diffs {
            // The review runs over this diff: one, or more when a run asked
            // only the groups that gave the first no answer.
            let over: Vec<i64> = runs
                .iter()
                .filter(|r| r.kind == "review" && r.base == base && r.head == head)
                .map(|r| r.id)
                .collect();
            let on_diff = |run: i64| {
                runs.iter()
                    .any(|r| r.id == run && r.base == base && r.head == head)
            };
            let label = format!("landing {landing} run {}", over[0]);
            let read = record::commits_of_run(&conn, over[0])?;
            let diff = match diff_of(inputs.repo_dir, base, head, &read) {
                Ok(diff) => diff,
                Err(why) => {
                    text.push_str(&format!("{label} ({key}): not replayed: {}\n", why.text()));
                    failed = true;
                    continue;
                }
            };
            let picked: Vec<_> = material
                .groups
                .pick(&material.rules, &diff.changed)
                .into_iter()
                .filter(|(g, _)| {
                    inputs.groups.is_empty() || inputs.groups.contains(&g.name.as_str())
                })
                .collect();
            if picked.is_empty() {
                text.push_str(&format!(
                    "{label} ({key}): no group asked covers a changed path\n"
                ));
                continue;
            }
            let asked: Vec<String> = picked
                .iter()
                .flat_map(|(group, members)| prompt::answer_names(group, members))
                .collect();
            let asks = |rule: &str| asked.iter().any(|a| a == rule);
            let new_calls = run::ask_groups(&material, inputs.reviewer, &picked, &diff, None).calls;
            let stamp = inputs.stamp;
            let (run_id, now_calls, now_findings, now_verdicts) =
                inputs
                    .store
                    .write(stamp, &format!("review replay {label}"), |db| {
                        let (id, _) = record::add_run(
                            db,
                            &NewRun {
                                landing: None,
                                kind: Kind::Replay,
                                label: Some(&label),
                                at: stamp.at,
                                base: &diff.base,
                                head: &diff.head,
                                commits: &[],
                                grouping: &material.grouping,
                                rules_version: &rules_version,
                            },
                            &new_calls,
                        )?;
                        Ok((
                            id,
                            record::calls_of_run(db, id)?,
                            record::findings_of_run(db, id)?,
                            record::verdicts_of_run(db, id)?,
                        ))
                    })?;
            // The settled findings over this diff in the rules the replay asked:
            // the review's, and the audits' candidate misses.
            let truths: Vec<&Finding> = findings
                .iter()
                .filter(|f| f.kind == "review" && f.counts() && f.settled.is_some())
                .filter(|f| over.contains(&f.run_id))
                .chain(misses.iter().copied().filter(|m| on_diff(m.run_id)))
                .filter(|f| asks(&f.rule))
                .collect();
            let new = score::count_replay(
                &mut replayed,
                &now_calls,
                &now_verdicts,
                &now_findings,
                &truths,
                &rules_version,
            );
            let old_calls: Vec<&Call> = calls
                .iter()
                .filter(|c| c.kind == "review" && over.contains(&c.run_id))
                .filter(|c| c.rules.iter().any(|r| asks(r)))
                .collect();
            let old_verdicts: Vec<Verdict> =
                verdicts.iter().filter(|v| asks(&v.rule)).cloned().collect();
            let old_findings: Vec<&Finding> = findings
                .iter()
                .filter(|f| f.kind == "review" && f.counts() && over.contains(&f.run_id))
                .filter(|f| asks(&f.rule))
                .collect();
            let confirmed: Vec<&Finding> = misses
                .iter()
                .copied()
                .filter(|m| on_diff(m.run_id) && m.settled_as("confirmed") && asks(&m.rule))
                .collect();
            score::count_settled(
                &mut in_use,
                &runs,
                &old_calls,
                &old_verdicts,
                &old_findings,
                &confirmed,
            );
            text.push_str(&format!(
                "{label} ({key}) over {}...{}: replay run {run_id}, {} settled findings to pair, {} new\n",
                short(&diff.base),
                short(&diff.head),
                truths.len(),
                new.len()
            ));
            for call in now_calls.iter().filter(|c| c.failure.is_some()) {
                text.push_str(&format!(
                    "  group {} failed: {}\n",
                    call.group,
                    call.failure.as_deref().unwrap_or("")
                ));
                failed = true;
            }
            for finding in new {
                text.push_str(&render::finding_text(finding));
            }
        }
    }
    text.push_str(&score::comparison(&in_use, &replayed));
    Ok(Replayed { text, failed })
}

fn short(rev: &str) -> &str {
    &rev[..rev.len().min(10)]
}

/// The diff a run read: <base>...<head> in <repo> when it has both commits,
/// else the commits of <repo>'s history that carry <read>'s patch ids. The
/// reviewers read a copy of the files at its head.
fn diff_of(repo: &Path, base: &str, head: &str, read: &[Commit]) -> Result<Diff> {
    let (base, head) = if commits::exists(repo, base) && commits::exists(repo, head) {
        (base.to_string(), head.to_string())
    } else {
        landed(repo, read)?
    };
    let mut diff = run::diff_between(repo, &base, &head)?;
    if let Some(source) = &mut diff.source {
        source.in_place = false;
    }
    Ok(diff)
}

/// The base and head of the commits among <repo>'s newest that carry <read>'s
/// patch ids, in order and one after another.
fn landed(repo: &Path, read: &[Commit]) -> Result<(String, String)> {
    if read.is_empty() {
        fail!(
            "the repo does not have the run's commits, and the run recorded none to find by patch id"
        );
    }
    let recent = commits::with_patch_ids(repo, &commits::newest(repo, SEARCHED)?)?;
    let start = recent
        .windows(read.len())
        .position(|window| commits::same_change(window, read));
    match start {
        Some(start) => Ok((
            format!("{}^", recent[start].sha),
            recent[start + read.len() - 1].sha.clone(),
        )),
        None => fail!(
            "the repo does not have the run's commits, and its newest {SEARCHED} commits hold no run of commits with the run's patch ids"
        ),
    }
}

#[cfg(test)]
#[path = "rerun_tests.rs"]
mod tests;
