//! The queue item that builds made for each stable Rust release
//! (rust_release.sql). `rust_stable.rs` finds a new release and the pins
//! that lag it; this records the item made for the release, in the same
//! write that adds the item, so that a project holds at most one item per
//! release.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::lists::item::{ItemDraft, add_item};
use crate::lists::{Id, Stamp};

/// The item made for stable release <version>, if one was.
pub fn queued(db: &Connection, version: &str) -> Result<Option<Id>> {
    db.query_row(
        "SELECT item_id FROM rust_release_item WHERE version = ?1",
        [version],
        |row| row.get(0),
    )
    .optional()
    .sql()
}

/// Adds the item <draft> describes for release <version>, unless the
/// project already made one for it; returns the new item's id, or None when
/// one existed.
pub fn queue_once(
    db: &Connection,
    version: &str,
    draft: &ItemDraft,
    stamp: &Stamp,
) -> Result<Option<Id>> {
    if queued(db, version)?.is_some() {
        return Ok(None);
    }
    let id = add_item(db, draft, None, stamp)?;
    db.execute(
        "INSERT INTO rust_release_item (version, item_id) VALUES (?1, ?2)",
        params![version, id],
    )
    .sql()?;
    Ok(Some(id))
}

#[cfg(test)]
#[path = "rust_release_tests.rs"]
mod tests;
