//! What a failed step of an app build said (app_build_step_failure.sql): an
//! excerpt of its output and, for a shots run, the shots that fail it.
//! app_build.rs reads each line of a step's output as it passes through
//! (`StepRead`), and app_timing.rs calls `record` in the transaction that
//! writes the run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, Transaction, params};

use crate::app_timing::StepRecord;
use crate::db::Sql;
use crate::fail::{Context, Result};
use crate::lock::EachLine;
use crate::output_excerpt::Excerpt;
use crate::shot_findings::failing_shots;

/// What a step that failed or ran past its limit said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepFailure {
    /// The lines of its output that say why (output_excerpt.rs); None when
    /// it printed nothing.
    pub excerpt: Option<String>,
    /// The shots its shots program said fail the run, sorted.
    pub shots: Vec<String>,
}

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
        let mut shots = read.shots.clone();
        shots.sort();
        shots.dedup();
        StepFailure {
            excerpt: read.excerpt.text(),
            shots,
        }
    }
}

/// Writes what each failed step of <steps> said, as the steps of run
/// <run_id>, by their position.
pub fn record(tx: &Transaction, run_id: i64, steps: &[StepRecord]) -> Result<()> {
    let what = || "writing what the failed steps said".to_string();
    let mut excerpt = tx
        .prepare(
            "INSERT INTO app_build_step_excerpt (run_id, position, excerpt) VALUES (?1, ?2, ?3)",
        )
        .context(what)?;
    let mut shot = tx
        .prepare(
            "INSERT INTO app_build_step_failed_shot (run_id, position, shot) VALUES (?1, ?2, ?3)",
        )
        .context(what)?;
    for (position, step) in steps.iter().enumerate() {
        let Some(failure) = &step.failure else {
            continue;
        };
        if let Some(text) = &failure.excerpt {
            excerpt
                .execute(params![run_id, position as i64, text])
                .context(what)?;
        }
        for name in &failure.shots {
            shot.execute(params![run_id, position as i64, name])
                .context(what)?;
        }
    }
    Ok(())
}

/// What each failed step of run <run_id> that has rows said, by its
/// position.
pub fn read(db: &Connection, run_id: i64) -> Result<HashMap<u32, StepFailure>> {
    let mut said: HashMap<u32, StepFailure> = HashMap::new();
    let mut excerpts = db
        .prepare("SELECT position, excerpt FROM app_build_step_excerpt WHERE run_id = ?1")
        .sql()?;
    let rows = excerpts
        .query_map([run_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?;
    for row in rows {
        let (position, excerpt) = row.sql()?;
        said.entry(position as u32).or_default().excerpt = Some(excerpt);
    }
    let mut shots = db
        .prepare(
            "SELECT position, shot FROM app_build_step_failed_shot WHERE run_id = ?1 ORDER BY position, shot",
        )
        .sql()?;
    let rows = shots
        .query_map([run_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?;
    for row in rows {
        let (position, name) = row.sql()?;
        said.entry(position as u32).or_default().shots.push(name);
    }
    Ok(said)
}

#[cfg(test)]
#[path = "app_build_step_failure_tests.rs"]
mod tests;
