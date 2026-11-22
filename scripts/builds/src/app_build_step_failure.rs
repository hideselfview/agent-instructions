//! What a failed step of an app build said (app_build_step_failure.sql): an
//! excerpt of its output and, for a shots run, the shots that fail it
//! (step_failure.rs).
//! app_build.rs reads each line of a step's output as it passes through
//! (`StepRead`), and app_timing.rs calls `record` in the transaction that
//! writes the run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, Transaction};

use crate::app_timing::StepRecord;
use crate::fail::Result;
use crate::lock::EachLine;
use crate::output_excerpt::Excerpt;
use crate::shot_findings::failing_shots;
use crate::step_failure::{StepFailure, Tables};

const TABLES: Tables = Tables {
    insert_failure: "INSERT INTO app_build_step_failure (run_id, position, excerpt) VALUES (?1, ?2, ?3)",
    insert_failed: "INSERT INTO app_build_step_failed_shot (run_id, position, shot) VALUES (?1, ?2, ?3)",
    select_failures: "SELECT position, excerpt FROM app_build_step_failure WHERE run_id = ?1",
    select_failed: "SELECT position, shot FROM app_build_step_failed_shot WHERE run_id = ?1",
};

/// What a `StepRead` read so far.
#[derive(Default)]
struct Read {
    excerpt: Excerpt,
    shots: Vec<String>,
}

/// Reads each line of a step's output, from both streams. Clones share what
/// they read.
#[derive(Clone, Default)]
pub struct StepRead(Arc<Mutex<Read>>);

impl StepRead {
    /// Reads one line, without its newline.
    pub fn line(&self, text: &str) {
        let mut read = self.0.lock().expect("no panic holding it");
        read.excerpt.line(text);
        if let Some(shots) = failing_shots(text) {
            read.shots.extend(shots);
        }
    }

    /// The reader a run gives each line of the step's output to.
    pub fn each_line(&self) -> EachLine {
        let read = self.clone();
        Arc::new(move |text: &str| read.line(text))
    }

    /// What the step said, for a step that failed or ran past its limit.
    pub fn failure(&self) -> StepFailure {
        let read = self.0.lock().expect("no panic holding it");
        StepFailure::new(read.excerpt.text(), read.shots.clone())
    }
}

/// Writes what each failed step of <steps> said, as the steps of run
/// <run_id>, by their position.
pub fn record(tx: &Transaction, run_id: i64, steps: &[StepRecord]) -> Result<()> {
    crate::step_failure::record(
        tx,
        &TABLES,
        run_id,
        steps
            .iter()
            .enumerate()
            .filter_map(|(position, step)| Some((position, step.result.failure()?))),
    )
}

/// What each failed step of run <run_id> said, by its position, when
/// builds kept it.
pub fn read(db: &Connection, run_id: i64) -> Result<HashMap<u32, StepFailure>> {
    crate::step_failure::read(db, &TABLES, run_id)
}

/// Removes every failure row from the store at <path>, so a step that failed
/// has no failure row: the damage a test of reading a run needs, which
/// nothing else can write.
#[cfg(test)]
pub fn remove_every_failure(path: &std::path::Path) -> crate::fail::Result<()> {
    use crate::db::Sql;
    Connection::open(path)
        .sql()?
        .execute("DELETE FROM app_build_step_failure", [])
        .sql()?;
    Ok(())
}

#[cfg(test)]
#[path = "app_build_step_failure_tests.rs"]
mod tests;
