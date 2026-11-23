//! The record of every `builds land` run that reached a project's lists
//! (land_run.sql). land.rs times each step of the run and, once the landing
//! ends and before land fast-forwards the main checkout, whose post-merge
//! hook in agent-instructions may install a builds that moves the store, the
//! program writes the run here, in one transaction, while the landing still
//! holds its use of the stores (install_gate.rs). A write that fails
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
    /// The checks of the lists and of the review, and of the branch's app
    /// builds, before anything is fetched.
    ListsCheck,
    Fetch,
    /// The fast-forward or cherry-pick onto origin/main.
    Pick,
    /// The whole-tree checks and the compile check of the result.
    Checks,
    Push,
    /// The records after the push.
    Records,
    /// Leaving the worktree on the landed commit or the branch, and releasing
    /// it. The fast-forward of the main checkout comes after the run's record,
    /// so no step times it.
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

/// Writes <run> to the store database open as <conn>, in one transaction.
pub fn write(conn: &mut Connection, run: &LandRun) -> Result<()> {
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

/// The most recent landing attempt for each branch, when the run ended.
pub fn branch_ends(db: &Connection) -> Result<Vec<(String, String, i64)>> {
    let mut statement = db
        .prepare(
            "SELECT repo, branch, MAX(started_at + ms / 1000) FROM land_run
         WHERE branch IS NOT NULL GROUP BY repo, branch",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// Makes `land_step` again as land_run.sql writes it, with the kinds of the
/// wait for the turn of a landing that changes the apps and of the app build
/// of the result, which a store from before allowed neither; each step keeps
/// its row.
pub fn allow_app_steps(db: &Connection) -> Result<()> {
    crate::db::remake_table(
        db,
        "land_step",
        crate::db::statement(include_str!("land_run.sql"), "CREATE TABLE land_step", ";"),
        |db| {
            db.execute_batch(
                "INSERT INTO land_step (land_id, position, kind, started_at, ms) SELECT land_id, position, kind, started_at, ms FROM land_step_old",
            )
            .sql()
        },
        &[],
    )
}

#[cfg(test)]
#[path = "land_run_tests.rs"]
mod tests;
