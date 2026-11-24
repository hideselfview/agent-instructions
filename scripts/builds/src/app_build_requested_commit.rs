//! The commit an app build was asked to build. app_timing.rs writes it in the
//! transaction that writes the run and names the store in the error.

use rusqlite::{Connection, params};

/// Records that app build run <run> was asked to build <commit>.
pub fn record(db: &Connection, run: i64, commit: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO app_build_requested_commit (run_id, requested_commit) VALUES (?1, ?2)",
        params![run, commit],
    )?;
    Ok(())
}
