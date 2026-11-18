//! How the last check for a new stable Rust release ended in each project
//! (rust_check.sql). The daemon's check (rust_stable.rs) records its outcome
//! here, and `builds health` lists each fault until a later check succeeds.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::health::Advice;
use crate::lists::store::stores;
use crate::places::Places;
use crate::text::local_minute;

/// Records that the check at <at> ended with <fault>, or with none.
pub fn record(db: &Connection, at: i64, fault: Option<&str>) -> Result<()> {
    db.execute(
        "INSERT INTO rust_check (singleton, at, fault) VALUES (1, ?1, ?2) \
         ON CONFLICT (singleton) DO UPDATE SET at = excluded.at, fault = excluded.fault",
        params![at, fault],
    )
    .sql()?;
    Ok(())
}

/// When the last check ended and its fault, if it had one.
pub fn read(db: &Connection) -> Result<Option<(i64, Option<String>)>> {
    db.query_row("SELECT at, fault FROM rust_check", [], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })
    .optional()
    .sql()
}

/// What `builds health` says: a finding for each project whose last check
/// for a new stable Rust release ended with a fault, and for each project
/// whose record of the check can't be read, naming the error.
pub fn findings(places: &Places) -> Vec<Advice> {
    let stores = match stores(places) {
        Ok(stores) => stores,
        Err(error) => {
            return vec![Advice::said(format!(
                "rust: builds could not list the stores to read the last stable Rust checks: {}",
                error.text()
            ))];
        }
    };
    stores
        .iter()
        .filter_map(|store| {
            let read = store.open_existing().and_then(|db| read(&db));
            match read {
                Ok(Some((at, Some(fault)))) => Some(Advice::said(format!(
                    "rust: project {}'s check for a new stable Rust release failed at {}; it repeats every hour until it succeeds: {fault}",
                    store.project,
                    local_minute(at)
                ))),
                Ok(_) => None,
                Err(error) => Some(Advice::said(format!(
                    "rust: builds could not read project {}'s last check for a new stable Rust release: {}",
                    store.project,
                    error.text()
                ))),
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "rust_check_tests.rs"]
mod tests;
