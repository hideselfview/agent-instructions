//! What a failed step said: an excerpt of its output (output_excerpt.rs) and
//! the names it failed, such as the staged files a hook check failed on
//! (hook_check_failure.rs) or the shots an app build's shots run failed
//! (app_build_step_failure.rs). Each of those owns its tables and passes its
//! own statements to `record` and `read`.

use std::collections::HashMap;

use rusqlite::{Connection, Transaction, params};

use crate::db::Sql;
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

/// The statements of the tables that keep one kind of step's failures, by
/// the run's id and the step's position in it. Each inserts or selects
/// `(run_id, position, <value>)`: the excerpt, or one failed name.
pub struct Tables {
    pub insert_excerpt: &'static str,
    pub insert_failed: &'static str,
    /// Selects `position, excerpt` of a run's steps, given its id.
    pub select_excerpts: &'static str,
    /// Selects `position, <name>` of a run's steps, given its id.
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
    let mut excerpt = tx.prepare(tables.insert_excerpt).context(what)?;
    let mut failed = tx.prepare(tables.insert_failed).context(what)?;
    for (position, failure) in failures {
        if let Some(text) = &failure.excerpt {
            excerpt
                .execute(params![run_id, position as i64, text])
                .context(what)?;
        }
        for name in &failure.failed {
            failed
                .execute(params![run_id, position as i64, name])
                .context(what)?;
        }
    }
    Ok(())
}

/// What each failed step of run <run_id> that has rows said, by its
/// position.
pub fn read(db: &Connection, tables: &Tables, run_id: i64) -> Result<HashMap<u32, StepFailure>> {
    let mut said: HashMap<u32, StepFailure> = HashMap::new();
    for (position, excerpt) in rows(db, tables.select_excerpts, run_id)? {
        said.entry(position).or_default().excerpt = Some(excerpt);
    }
    for (position, name) in rows(db, tables.select_failed, run_id)? {
        said.entry(position).or_default().failed.push(name);
    }
    for failure in said.values_mut() {
        failure.failed.sort();
    }
    Ok(said)
}

/// The `(position, text)` rows that <select> gives for run <run_id>.
fn rows(db: &Connection, select: &str, run_id: i64) -> Result<Vec<(u32, String)>> {
    let mut statement = db.prepare(select).sql()?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((row.get::<_, i64>(0)? as u32, row.get::<_, String>(1)?))
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}
