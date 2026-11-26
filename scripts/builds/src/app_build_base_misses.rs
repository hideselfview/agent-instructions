//! Why the merge base's saved run was missing for each shots step that built
//! it (app_build_base_misses.sql). app_build_step_shots.rs calls `record` in
//! the transaction that writes the step's row, and reads the causes back
//! with the step's other facts.

use rusqlite::{Transaction, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::shots_cache::MissCause;

/// Writes <cause> for step <position> of run <run_id>. A cause the store
/// keeps no row of (`MissCause::Unrecorded`) writes nothing.
pub fn record(tx: &Transaction, run_id: i64, position: i64, cause: MissCause) -> Result<()> {
    let Some(word) = cause.word() else {
        return Ok(());
    };
    tx.execute(
        "INSERT INTO app_build_base_misses (run_id, position, cause) VALUES (?1, ?2, ?3)",
        params![run_id, position, word],
    )
    .sql()?;
    Ok(())
}
