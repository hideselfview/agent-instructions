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

#[cfg(test)]
#[path = "exchange_time_tests.rs"]
mod tests;
