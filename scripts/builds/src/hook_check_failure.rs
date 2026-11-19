//! What a failed check of a hook run said (hook_check_failure.sql): an
//! excerpt of its output and the staged files it failed on. checks_run.rs
//! reads them from the check's output, and hook_stats.rs calls `record` in
//! the transaction that writes the run.

use std::collections::HashMap;

use rusqlite::{Connection, Transaction, params};

use crate::db::Sql;
use crate::fail::{Context, Result};
use crate::hook_stats::CheckRecord;
use crate::output_excerpt::Excerpt;

/// What a failed check said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckFailure {
    /// The lines of its output that say why (output_excerpt.rs); None when
    /// it printed nothing.
    pub excerpt: Option<String>,
    /// The files of those it was handed that its output names, sorted.
    pub files: Vec<String>,
}

impl CheckFailure {
    /// What a check that was handed <handed> and printed <output> said.
    pub fn of(output: &str, handed: &[String]) -> CheckFailure {
        let mut files: Vec<String> = handed
            .iter()
            .filter(|file| output.contains(file.as_str()))
            .cloned()
            .collect();
        files.sort();
        files.dedup();
        CheckFailure {
            excerpt: Excerpt::of(output),
            files,
        }
    }
}

/// Writes what each failed check of <checks> said, as the checks of run
/// <run_id>, by their position.
pub fn record(tx: &Transaction, run_id: i64, checks: &[CheckRecord]) -> Result<()> {
    let what = || "writing what the failed checks said".to_string();
    let mut excerpt = tx
        .prepare("INSERT INTO hook_check_excerpt (run_id, position, excerpt) VALUES (?1, ?2, ?3)")
        .context(what)?;
    let mut file = tx
        .prepare("INSERT INTO hook_check_failed_file (run_id, position, path) VALUES (?1, ?2, ?3)")
        .context(what)?;
    for (position, check) in checks.iter().enumerate() {
        let Some(failure) = check.outcome.failure() else {
            continue;
        };
        if let Some(text) = &failure.excerpt {
            excerpt
                .execute(params![run_id, position as i64, text])
                .context(what)?;
        }
        for path in &failure.files {
            file.execute(params![run_id, position as i64, path])
                .context(what)?;
        }
    }
    Ok(())
}

/// What each failed check of run <run_id> that has rows said, by its
/// position.
pub fn read(db: &Connection, run_id: i64) -> Result<HashMap<u32, CheckFailure>> {
    let mut said: HashMap<u32, CheckFailure> = HashMap::new();
    let mut excerpts = db
        .prepare("SELECT position, excerpt FROM hook_check_excerpt WHERE run_id = ?1")
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
    let mut files = db
        .prepare(
            "SELECT position, path FROM hook_check_failed_file WHERE run_id = ?1 ORDER BY position, path",
        )
        .sql()?;
    let rows = files
        .query_map([run_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?;
    for row in rows {
        let (position, path) = row.sql()?;
        said.entry(position as u32).or_default().files.push(path);
    }
    Ok(said)
}

#[cfg(test)]
#[path = "hook_check_failure_tests.rs"]
mod tests;
