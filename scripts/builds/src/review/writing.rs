//! `builds review writing`: asks the group that holds the rule
//! write-plain-literal-active-sentences over a worktree's net change past
//! its base, the way `builds review` asks each group (`run::ask_groups`), and
//! prints the findings. The check records no landing, review run or finding,
//! so an implementing agent runs it on its own branch before it reports, and
//! fixes what it finds. builds records each writing check whose group
//! answered (`writing.sql`), and refuses a third writing check on a branch
//! until the coordinator reviews the branch again.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::Stamp;
use crate::lists::store::Store;
use crate::review::record;
use crate::review::render;
use crate::review::run;

/// The rule whose group the writing check asks.
pub const WRITING_RULE: &str = "write-plain-literal-active-sentences";

/// How many writing checks builds runs on a branch between two reviews of it.
pub const RUNS_PER_REVIEW: i64 = 2;

/// What one writing check needs.
pub struct Inputs<'a> {
    pub store: &'a Store,
    pub project: &'a str,
    pub repo: &'a str,
    pub worktree: &'a Path,
    pub base: &'a str,
    /// Names the change, as the key of its landing under review does.
    pub key: &'a str,
    pub root: &'a Path,
    pub reviewer: &'a Path,
    pub stamp: &'a Stamp,
}

/// What the writing check found.
#[derive(Debug)]
pub struct Checked {
    /// Everything to print.
    pub text: String,
    /// Whether the group answered and raised no finding that builds kept.
    pub clean: bool,
}

/// Asks the group of the rules checkout that holds `WRITING_RULE`, with the
/// project's rules, over the net change of the worktree's HEAD past the
/// base, and records the run when the group answered. Fails without asking
/// when the key has `RUNS_PER_REVIEW` runs since its last review.
pub fn check(inputs: &Inputs) -> Result<Checked> {
    let material = run::Material::load(inputs.root, inputs.project)?;
    let Some(group) = material
        .groups
        .groups
        .iter()
        .find(|group| group.rules.iter().any(|rule| rule == WRITING_RULE))
    else {
        fail!(
            "no group of {} holds rule {WRITING_RULE}",
            inputs.root.display()
        );
    };
    let diff = run::diff_of(inputs.worktree, inputs.base)?.diff;
    if diff.text.trim().is_empty() {
        return Ok(Checked {
            text: format!(
                "there are no changes past {}, so there is nothing to check\n",
                inputs.base
            ),
            clean: true,
        });
    }
    let picked: Vec<_> = material
        .groups
        .pick(&material.rules, &diff.changed)
        .into_iter()
        .filter(|(picked, _)| picked.name == group.name)
        .collect();
    if picked.is_empty() {
        return Ok(Checked {
            text: format!(
                "{}: its rules cover none of the changed paths, so there is nothing to check\n",
                group.name
            ),
            clean: true,
        });
    }
    let conn = inputs.store.open_existing()?;
    let done = runs_since_review(&conn, inputs.repo, inputs.key)?;
    drop(conn);
    if done >= RUNS_PER_REVIEW {
        fail!(
            "builds review writing ran {done} times on {} since its last review, and runs at most {RUNS_PER_REVIEW} times between reviews: fix what the second run found, and report",
            inputs.key
        );
    }
    let asked = run::ask_groups(&material, inputs.reviewer, &picked, &diff, None, &[]);
    let answered = asked.calls.iter().all(|call| call.failure.is_none());
    let clean = answered
        && asked
            .calls
            .iter()
            .all(|call| call.findings.iter().all(|f| f.check_failed.is_some()));
    let mut text = render::unrecorded_report(&asked.calls);
    if answered {
        let stamp = inputs.stamp;
        let run = inputs
            .store
            .write(stamp, &format!("review writing {}", inputs.key), |db| {
                add_run(db, inputs.repo, inputs.key, stamp.at)?;
                runs_since_review(db, inputs.repo, inputs.key)
            })?;
        text.push_str(&format!(
            "builds ran writing check {run} of {RUNS_PER_REVIEW} on {} since its last review\n",
            inputs.key
        ));
    }
    Ok(Checked { text, clean })
}

/// How many writing checks ran on <key> in <repo> since the last review run
/// of a landing with that key, or ever, when no review ran on it.
fn runs_since_review(db: &Connection, repo: &str, key: &str) -> Result<i64> {
    let reviewed = record::last_review_run(db, repo, key)?;
    db.query_row(
        "SELECT COUNT(*) FROM review_writing_run WHERE repo = ?1 AND key = ?2 AND reviewed_run IS ?3",
        params![repo, key, reviewed],
        |row| row.get(0),
    )
    .sql()
}

/// Records a writing check on <key> in <repo>, after the key's last review
/// run.
pub fn add_run(db: &Connection, repo: &str, key: &str, at: i64) -> Result<()> {
    let reviewed = record::last_review_run(db, repo, key)?;
    db.execute(
        "INSERT INTO review_writing_run (key, repo, reviewed_run, at) VALUES (?1, ?2, ?3, ?4)",
        params![key, repo, reviewed, at],
    )
    .sql()?;
    Ok(())
}

#[cfg(test)]
#[path = "writing_tests.rs"]
mod tests;
