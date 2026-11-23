//! The time of each reviewer process (exchange_time.sql). record.rs writes a
//! row with each exchange it stores.

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;

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

/// When the last reviewer process of the run finished. Runs that recorded
/// no exchange times have only their start time.
pub fn run_finished_at(db: &Connection, run: i64) -> Result<i64> {
    db.query_row(
        &format!(
            "SELECT COALESCE((
                SELECT MAX({END_AT})
                FROM review_call c
                JOIN review_exchange e ON e.call_id = c.id
                JOIN review_exchange_time t ON t.exchange_id = e.id
                WHERE c.run_id = r.id
            ), r.at) FROM review_run r WHERE r.id = ?1"
        ),
        [run],
        |row| row.get(0),
    )
    .sql()
}

#[cfg(test)]
#[path = "exchange_time_tests.rs"]
mod tests;
