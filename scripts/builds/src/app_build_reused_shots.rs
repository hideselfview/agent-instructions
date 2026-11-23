//! The cached shots comparisons a lane reused for the commit it built.
//! app_timing.rs records them in the transaction that writes the lane run.

use rusqlite::{Connection, Transaction, params};
use shots::Prefix;

use crate::db::Sql;
use crate::fail::{Context, Result};

pub fn record(tx: &Transaction, run: i64, prefixes: &[Prefix]) -> Result<()> {
    let mut insert = tx
        .prepare(
            "INSERT INTO app_build_reused_shots (run_id, prefix) VALUES (?1, ?2)
             ON CONFLICT (run_id, prefix) DO NOTHING",
        )
        .sql()?;
    for prefix in prefixes {
        insert.execute(params![run, prefix.as_str()]).sql()?;
    }
    Ok(())
}

pub fn read(conn: &Connection, run: i64) -> Result<Vec<Prefix>> {
    let mut statement = conn
        .prepare("SELECT prefix FROM app_build_reused_shots WHERE run_id = ?1 ORDER BY prefix")
        .sql()?;
    let rows = statement
        .query_map([run], |row| row.get::<_, String>(0))
        .sql()?;
    rows.map(|row| {
        let prefix = row.sql()?;
        Prefix::parse(&prefix).context(|| format!("reading a reused shots prefix for run {run}"))
    })
    .collect()
}
