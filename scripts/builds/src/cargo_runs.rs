//! The record of `builds cargo` runs: which host ran each and when (the
//! schema is cargo_runs.sql). A run writes its row when it starts and
//! finishes it when cargo ends; the dashboard's host rows read what is
//! running and how much ran lately.
//!
//! A write that fails is a warning and changes nothing about the build's
//! result, as with the app-build record. A row of a run that died without
//! ending counts as running only while its process is alive, so a killed
//! `builds` shows nothing wrong for longer than it takes the dashboard to
//! look.

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

/// A run that is running now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    pub started_at: i64,
    pub repo: String,
    pub checkout: String,
    pub host: String,
    pub command: String,
}

/// The runs in the database at <path> that have not ended and whose process
/// is alive; empty when nothing was recorded.
pub fn running(path: &Path) -> Result<Vec<Running>> {
    let Some(conn) = db::open_reader(path)? else {
        return Ok(Vec::new());
    };
    let mut statement = conn
        .prepare("SELECT started_at, repo, checkout, host, command, pid FROM cargo_runs WHERE ended_at IS NULL ORDER BY started_at, id")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                Running {
                    started_at: row.get(0)?,
                    repo: row.get(1)?,
                    checkout: row.get(2)?,
                    host: row.get(3)?,
                    command: row.get(4)?,
                },
                row.get::<_, i64>(5)?,
            ))
        })
        .sql()?;
    let mut found = Vec::new();
    for row in rows {
        let (run, pid) = row.sql()?;
        if is_alive(pid) {
            found.push(run);
        }
    }
    Ok(found)
}

/// How many runs each host started at or after <since> (seconds since 1970)
/// in the database at <path>, by host name.
pub fn started_since(path: &Path, since: i64) -> Result<Vec<(String, u32)>> {
    let Some(conn) = db::open_reader(path)? else {
        return Ok(Vec::new());
    };
    let mut statement = conn
        .prepare("SELECT host, COUNT(*) FROM cargo_runs WHERE started_at >= ?1 GROUP BY host ORDER BY host")
        .sql()?;
    let rows = statement
        .query_map(params![since], |row| Ok((row.get(0)?, row.get(1)?)))
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// Does process <pid> exist? A process of another user exists too.
fn is_alive(pid: i64) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    unsafe { libc::kill(pid, 0) == 0 || *libc::__error() == libc::EPERM }
}

#[cfg(test)]
#[path = "cargo_runs_tests.rs"]
mod tests;
