//! hook_check_starts.sql stores this Mac's core count and load at the moment
//! each check of a hook run started. hook_stats.rs calls `record` here in the
//! transaction that writes the run, and reads the rows back with its checks.

use rusqlite::{Transaction, params};

use crate::fail::{Context, Result};
use crate::hook_stats::CheckRecord;

/// Writes the starting load of each check of <checks> that has one, as the
/// checks of run <run_id>, by their position.
pub fn record(tx: &Transaction, run_id: i64, checks: &[CheckRecord]) -> Result<()> {
    let what = || "writing the checks' starting loads".to_string();
    let mut start = tx
        .prepare(
            "INSERT INTO hook_check_starts (run_id, position, cores, load) VALUES (?1, ?2, ?3, ?4)",
        )
        .context(what)?;
    for (position, check) in checks.iter().enumerate() {
        if let Some(load) = check.outcome.started() {
            start
                .execute(params![
                    run_id,
                    position as i64,
                    load.cores,
                    load.one_minute
                ])
                .context(what)?;
        }
    }
    Ok(())
}
