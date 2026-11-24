//! The commit an app build checked out. app_timing.rs writes it in the
//! transaction that writes the run and names the store in the error.

use rusqlite::{Connection, params};

/// Records that app build run <run> built <commit>.
pub fn record(db: &Connection, run: i64, commit: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO app_build_run_commit (run_id, built_commit) VALUES (?1, ?2)",
        params![run, commit],
    )?;
    Ok(())
}
