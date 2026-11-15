//! The record of `builds cargo` runs: which host ran each and when (the
//! schema is cargo_runs.sql). A run writes its row when it starts and
//! finishes it when cargo ends.
//!
//! A write that fails is a warning and changes nothing about the build's
//! result, as with the app-build record.

use std::path::Path;

use rusqlite::params;

use crate::db::{self, Sql};
use crate::fail::Result;

pub use crate::app_timing::now;

/// How long a finished run's row is kept, seconds.
const KEPT_SECONDS: i64 = 7 * 24 * 60 * 60;

/// A run as it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    pub started_at: i64,
    pub repo: String,
    pub checkout: String,
    pub host: String,
    pub pid: u32,
    pub command: String,
}

/// Writes <start> as a run that has not ended and returns its id; deletes the
/// rows of runs that ended more than a week ago.
pub fn begin(path: &Path, start: &Start) -> Result<i64> {
    let conn = db::open(path, db::Make::Never)?;
    conn.execute(
        "DELETE FROM cargo_runs WHERE ended_at < ?1",
        params![start.started_at - KEPT_SECONDS],
    )
    .sql()?;
    conn.execute(
        "INSERT INTO cargo_runs (started_at, repo, checkout, host, pid, command) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            start.started_at,
            start.repo,
            start.checkout,
            start.host,
            start.pid,
            start.command
        ],
    )
    .sql()?;
    Ok(conn.last_insert_rowid())
}

/// Finishes run <id> with the time it ended and cargo's exit status.
pub fn end(path: &Path, id: i64, ended_at: i64, exit_code: i32) -> Result<()> {
    let conn = db::open(path, db::Make::Never)?;
    conn.execute(
        "UPDATE cargo_runs SET ended_at = max(?2, started_at), exit_code = ?3 WHERE id = ?1 AND ended_at IS NULL",
        params![id, ended_at, exit_code],
    )
    .sql()?;
    Ok(())
}

#[cfg(test)]
#[path = "cargo_runs_tests.rs"]
mod tests;
