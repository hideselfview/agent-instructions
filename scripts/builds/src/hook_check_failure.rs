//! What a failed check of a hook run said (hook_check_failure.sql): an
//! excerpt of its output and the staged files it failed on
//! (step_failure.rs). checks_run.rs reads them from the check's output, and
//! hook_stats.rs calls `record` in the transaction that writes the run.

use std::collections::HashMap;

use rusqlite::{Connection, Transaction};

use crate::fail::Result;
use crate::hook_stats::CheckRecord;
use crate::output_excerpt::Excerpt;
use crate::step_failure::{StepFailure, Tables};

const TABLES: Tables = Tables {
    insert_excerpt: "INSERT INTO hook_check_excerpt (run_id, position, excerpt) VALUES (?1, ?2, ?3)",
    insert_failed: "INSERT INTO hook_check_failed_file (run_id, position, path) VALUES (?1, ?2, ?3)",
    select_excerpts: "SELECT position, excerpt FROM hook_check_excerpt WHERE run_id = ?1",
    select_failed: "SELECT position, path FROM hook_check_failed_file WHERE run_id = ?1",
};

/// What a check that was handed <handed> and printed <output> said: the
/// excerpt of its output, and the files of those it was handed that its
/// output names.
pub fn said(output: &str, handed: &[String]) -> StepFailure {
    StepFailure::new(
        Excerpt::of(output),
        handed
            .iter()
            .filter(|file| output.contains(file.as_str()))
            .cloned(),
    )
}

/// Writes what each failed check of <checks> said, as the checks of run
/// <run_id>, by their position.
pub fn record(tx: &Transaction, run_id: i64, checks: &[CheckRecord]) -> Result<()> {
    crate::step_failure::record(
        tx,
        &TABLES,
        run_id,
        checks
            .iter()
            .enumerate()
            .filter_map(|(position, check)| Some((position, check.outcome.failure()?))),
    )
}

/// What each failed check of run <run_id> that has rows said, by its
/// position.
pub fn read(db: &Connection, run_id: i64) -> Result<HashMap<u32, StepFailure>> {
    crate::step_failure::read(db, &TABLES, run_id)
}

#[cfg(test)]
#[path = "hook_check_failure_tests.rs"]
mod tests;
