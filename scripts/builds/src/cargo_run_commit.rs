//! The commit each `builds cargo` run ran on (cargo_run_commit.sql).
//! cargo_run.rs reads the worktree when the run starts (tree_state.rs) and
//! writes this with the run.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::tree_state::{TreeState, on_run_branch};

/// The branch of the item running in <worktree>, from the store <db>; None
/// when no item runs there.
fn run_branch(db: &Connection, worktree: &str) -> Result<Option<String>> {
    db.query_row(
        "SELECT branch FROM running_place WHERE worktree = ?1 LIMIT 1",
        [worktree],
        |row| row.get(0),
    )
    .optional()
    .sql()
}

/// Records that run <run>, in the worktree at <top>, ran on <state>, and
/// whether its commit was off the branch of the item running there. When
/// git cannot tell, builds says so and records the run's branch alone.
pub fn record(db: &Connection, run: i64, top: &Path, state: &TreeState) -> Result<()> {
    let run_branch = run_branch(db, &top.display().to_string())?;
    let off = match &run_branch {
        None => None,
        Some(run_branch) => {
            match on_run_branch(top, &state.head, state.branch.as_deref(), run_branch) {
                Ok(on) => Some(!on),
                Err(error) => {
                    crate::text::note(format!(
                        "builds could not tell whether this cargo run's commit is on branch {run_branch}: {error}"
                    ));
                    None
                }
            }
        }
    };
    db.execute(
        "INSERT INTO builds_cargo_run_commit (run_id, head, branch, changes, run_branch, off_branch) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![run, state.head, state.branch, state.changes, run_branch, off],
    )
    .sql()?;
    Ok(())
}
