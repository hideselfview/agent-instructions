//! An app build's requested commit and the commit it checked out. app_timing.rs
//! writes both in the transaction that writes the run and names the store in
//! the error.

use rusqlite::{Connection, params};

/// Records that app build run <run> built <commit>.
pub fn record(db: &Connection, run: i64, commit: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO app_build_run_commit (run_id, built_commit) VALUES (?1, ?2)",
        params![run, commit],
    )?;
    Ok(())
}

/// Records that app build run <run> was asked to build <commit>.
pub fn record_requested(db: &Connection, run: i64, commit: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO app_build_requested_commit (run_id, requested_commit) VALUES (?1, ?2)",
        params![run, commit],
    )?;
    Ok(())
}
