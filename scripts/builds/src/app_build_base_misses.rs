//! Why the merge base's saved run was missing for each shots step that built
//! it (app_build_base_misses.sql). app_build_step_shots.rs calls `record` in
//! the transaction that writes the step's row, and reads the causes back
//! with the step's other facts.

use rusqlite::{Transaction, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::shots_cache::MissCause;

/// Writes <cause> for step <position> of run <run_id>.
pub fn record(tx: &Transaction, run_id: i64, position: i64, cause: MissCause) -> Result<()> {
    tx.execute(
        "INSERT INTO app_build_base_misses (run_id, position, cause) VALUES (?1, ?2, ?3)",
        params![run_id, position, cause.word()],
    )
    .sql()?;
    Ok(())
}

/// Gives each built step a store holds from before builds kept the cause the
/// cause `unrecorded`: the store move that adds the table runs it.
pub fn mark_old_steps_unrecorded(tx: &Transaction) -> Result<()> {
    tx.execute_batch(
        "INSERT INTO app_build_base_misses (run_id, position, cause)
         SELECT run_id, position, 'unrecorded' FROM app_build_step_shots WHERE base_run = 'built'",
    )
    .sql()
}
