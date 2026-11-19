//! The record of every `builds land` run that reached a project's lists
//! (land_run.sql). land.rs times each step of the run and, when land ends,
//! the program writes the run here, in one transaction. A write that fails
//! is said on stderr and changes nothing about the landing's result, as with
//! the hook's and the app builds' records.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::lists::Id;

/// What a step of a landing is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// Waiting for an earlier landing in the worktree.
    Lock,
    /// The checks of the lists and of the review, before anything is fetched.
    ListsCheck,
    Fetch,
    /// The fast-forward or cherry-pick onto origin/main.
    Pick,
    /// The whole-tree checks and the compile check of the result.
    Checks,
    Push,
    /// The records after the push.
    Records,
    /// Releasing the worktree and fast-forwarding the main checkout.
    Finish,
}

impl StepKind {
    pub fn word(self) -> &'static str {
        match self {
            StepKind::Lock => "lock",
            StepKind::ListsCheck => "lists-check",
            StepKind::Fetch => "fetch",
            StepKind::Pick => "pick",
            StepKind::Checks => "checks",
            StepKind::Push => "push",
            StepKind::Records => "records",
            StepKind::Finish => "finish",
        }
    }
}

/// One step, as the run observed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    /// Seconds since 1970.
    pub started_at: i64,
    pub ms: u64,
}

/// One `builds land` run, as it goes into the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandRun {
    /// The project whose store holds the run.
    pub project: String,
    pub started_at: i64,
    pub repo: String,
    pub worktree: String,
    /// None for a detached HEAD.
    pub branch: Option<String>,
    pub by: String,
    pub ms: u64,
    pub landed: bool,
    /// The items the run finished or advanced.
    pub items: Vec<Id>,
    /// Each commit the run was asked to land, with its commit time.
    pub commits: Vec<(String, i64)>,
    pub steps: Vec<Step>,
}

/// Writes <run> to the store database at <path>, in one transaction.
pub fn record(path: &Path, run: &LandRun) -> Result<()> {
    let mut conn = crate::db::open(path, crate::db::Make::Never)?;
    write(&mut conn, run)
}

fn write(conn: &mut Connection, run: &LandRun) -> Result<()> {
    let tx = conn.transaction().sql()?;
    tx.execute(
        "INSERT INTO land_run (started_at, repo, worktree, branch, by, ms, landed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            run.started_at,
            run.repo,
            run.worktree,
            run.branch,
            run.by,
            run.ms as i64,
            run.landed
        ],
    )
    .sql()?;
    let id = tx.last_insert_rowid();
    for item in &run.items {
        tx.execute(
            "INSERT OR IGNORE INTO land_run_item (land_id, item_id) VALUES (?1, ?2)",
            params![id, item],
        )
        .sql()?;
    }
    for (position, (sha, committed_at)) in run.commits.iter().enumerate() {
        tx.execute(
            "INSERT INTO land_run_commit (land_id, position, sha, committed_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, position as i64, sha, committed_at],
        )
        .sql()?;
    }
    for (position, step) in run.steps.iter().enumerate() {
        tx.execute(
            "INSERT INTO land_step (land_id, position, kind, started_at, ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, position as i64, step.kind.word(), step.started_at, step.ms as i64],
        )
        .sql()?;
    }
    tx.commit().sql()
}

#[cfg(test)]
#[path = "land_run_tests.rs"]
mod tests;
