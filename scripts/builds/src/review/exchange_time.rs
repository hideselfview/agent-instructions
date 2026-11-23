//! The time of each reviewer process (exchange_time.sql). record.rs writes a
//! row with each exchange it stores.

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::{Fail, Result};

/// The end of an exchange, rounded up to a whole second. The row has alias `t`.
pub const END_AT: &str = "t.started_at + (t.ms + 999) / 1000";

/// Records that exchange <exchange> began at <started_at> (seconds since
/// 1970) and ran <ms> milliseconds.
pub fn record(db: &Connection, exchange: i64, started_at: i64, ms: i64) -> Result<()> {
    db.execute(
        "INSERT INTO review_exchange_time (exchange_id, started_at, ms) VALUES (?1, ?2, ?3)",
        params![exchange, started_at, ms],
    )
    .sql()?;
    Ok(())
}

/// The last exchange's end, at or after the run's start, for a
/// `review_run` row with alias `run`. SQL returns NULL when no time exists.
pub fn run_end_at() -> String {
    format!(
        "MAX(run.at, (SELECT MAX({END_AT}) FROM review_exchange_time t
         JOIN review_exchange e ON e.id = t.exchange_id
         JOIN review_call c ON c.id = e.call_id WHERE c.run_id = run.id))"
    )
}

/// The recorded finish of <run>, which must have timed exchanges.
pub fn run_finished_at(db: &Connection, run: i64) -> Result<i64> {
    let end = run_end_at();
    let at: Option<i64> = db
        .query_row(
            &format!("SELECT {end} FROM review_run run WHERE run.id = ?1"),
            [run],
            |row| row.get(0),
        )
        .sql()?;
    at.ok_or_else(|| {
        Fail::Message(format!(
            "Builds has no recorded finish time for review run {run}."
        ))
    })
}

#[cfg(test)]
pub(super) fn set_call_finish_for_finding(db: &Connection, finding: i64, at: i64) -> Result<()> {
    db.execute(
        "UPDATE review_exchange_time SET started_at = ?1, ms = 1000
         WHERE exchange_id IN (
             SELECT e.id FROM review_exchange e
             JOIN review_finding f ON f.call_id = e.call_id WHERE f.id = ?2
         )",
        params![at - 1, finding],
    )
    .sql()?;
    Ok(())
}

#[cfg(test)]
pub(super) fn clear(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM review_exchange_time", []).sql()?;
    Ok(())
}

#[cfg(test)]
#[path = "exchange_time_tests.rs"]
mod tests;
