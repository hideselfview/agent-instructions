//! What a failed step said: an excerpt of its output (output_excerpt.rs) and
//! the names it failed, such as the staged files a hook check failed on
//! (hook_check_failure.rs) or the shots an app build's shots run failed
//! (app_build_step_failure.rs). Each of those owns its tables and passes its
//! own statements to `record` and `read`.

use std::collections::HashMap;

use rusqlite::{Connection, Transaction, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::{Context, Result};

/// What a failed step said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepFailure {
    /// The lines of its output that say why; None when it printed nothing.
    pub excerpt: Option<String>,
    /// The names it failed, sorted, each once.
    pub failed: Vec<String>,
}

impl StepFailure {
    /// A failure that said <excerpt> and failed <names>, in any order.
    pub fn new(excerpt: Option<String>, names: impl IntoIterator<Item = String>) -> StepFailure {
        let mut failed: Vec<String> = names.into_iter().collect();
        failed.sort();
        failed.dedup();
        StepFailure { excerpt, failed }
    }
}

/// How a run of a step that passed or failed ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunResult {
    Passed,
    /// It failed, saying what its kind keeps; None for a run that builds
    /// recorded before it kept what failed runs said.
    Failed(Option<StepFailure>),
}

impl RunResult {
    pub fn passed(&self) -> bool {
        matches!(self, RunResult::Passed)
    }

    /// What a failed run said, when builds kept it.
    pub fn failure(&self) -> Option<&StepFailure> {
        match self {
            RunResult::Failed(failure) => failure.as_ref(),
            RunResult::Passed => None,
        }
    }
}

/// The statements of the tables that keep one kind of step's failures, by
/// the run's id and the step's position in it: a row for each failure with
/// its excerpt, and a row for each name it failed.
pub struct Tables {
    /// Inserts `(run_id, position, excerpt)`, the excerpt maybe NULL.
    pub insert_failure: &'static str,
    /// Inserts `(run_id, position, <name>)`.
    pub insert_failed: &'static str,
    /// Selects `position, excerpt` of a run's failures, given its id.
    pub select_failures: &'static str,
    /// Selects `position, <name>` of a run's failed names, given its id.
    pub select_failed: &'static str,
}

/// Writes each failure of <failures>, by the step's position, as the steps of
/// run <run_id>.
pub fn record<'a>(
    tx: &Transaction,
    tables: &Tables,
    run_id: i64,
    failures: impl Iterator<Item = (usize, &'a StepFailure)>,
) -> Result<()> {
    let what = || "writing what the failed steps said".to_string();
    let mut failure_row = tx.prepare(tables.insert_failure).context(what)?;
    let mut failed = tx.prepare(tables.insert_failed).context(what)?;
    for (position, failure) in failures {
        failure_row
            .execute(params![run_id, position as i64, failure.excerpt])
            .context(what)?;
        for name in &failure.failed {
            failed
                .execute(params![run_id, position as i64, name])
                .context(what)?;
        }
    }
    Ok(())
}

/// What each failed step of run <run_id> said, by its position: a step
/// with no failure row failed before builds kept what failed steps said.
pub fn read(db: &Connection, tables: &Tables, run_id: i64) -> Result<HashMap<u32, StepFailure>> {
    let mut said: HashMap<u32, StepFailure> = HashMap::new();
    let mut statement = db.prepare(tables.select_failures).sql()?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((
                row.get::<_, i64>(0)? as u32,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .sql()?;
    for row in rows {
        let (position, excerpt) = row.sql()?;
        said.insert(
            position,
            StepFailure {
                excerpt,
                failed: Vec::new(),
            },
        );
    }
    let mut statement = db.prepare(tables.select_failed).sql()?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((row.get::<_, i64>(0)? as u32, row.get::<_, String>(1)?))
        })
        .sql()?;
    for row in rows {
        let (position, name) = row.sql()?;
        let Some(failure) = said.get_mut(&position) else {
            fail!(
                "the store holds a failed name of step {position} of run {run_id}, which has no failure row"
            );
        };
        failure.failed.push(name);
    }
    for failure in said.values_mut() {
        failure.failed.sort();
    }
    Ok(said)
}
