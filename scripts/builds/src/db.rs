//! `builds.sqlite`, the one database in each project's store folder: the
//! lists (every table of `lists/*.sql`) and the hook-run stats
//! (`hook_stats.sql`). This module does all opening: the pragmas, the schema
//! made whole in the first transaction of a new file, and the stamp.
//!
//! The stamp is `PRAGMA user_version`, a hash of the assembled schema text. A
//! database whose stamp differs was made by another builds schema and is
//! refused; there is no version number and no chain of changes.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::fail;
use crate::fail::{Context, Result};

/// The database's file name in a project's store folder.
pub const DATABASE: &str = "builds.sqlite";

/// Each owner's SQL, in the order the schema is read: a table points at ones
/// earlier or later (SQLite resolves foreign keys when rows are written), but
/// the order is the one to read it in.
const LISTS_PARTS: [&str; 10] = [
    include_str!("lists/project.sql"),
    include_str!("lists/area.sql"),
    include_str!("lists/record.sql"),
    include_str!("lists/question.sql"),
    include_str!("lists/decision.sql"),
    include_str!("lists/item.sql"),
    include_str!("lists/running.sql"),
    include_str!("lists/landing.sql"),
    include_str!("lists/design.sql"),
    include_str!("lists/store.sql"),
];
const HOOK_PART: &str = include_str!("hook_stats.sql");

/// A busy store waits as `flock` did: until the other write ends. SQLite
/// takes the wait as milliseconds in an `i32`.
const BUSY: Duration = Duration::from_millis(i32::MAX as u64);

/// Every table of the lists, in schema order.
pub fn lists_schema() -> String {
    LISTS_PARTS.join("\n")
}

/// The whole schema: the lists' tables, then the hook stats'.
pub fn schema() -> String {
    format!("{}\n{HOOK_PART}", lists_schema())
}

/// The stamp of the schema text: its FNV-1a hash, folded into `user_version`'s
/// positive range and kept off 0 (no schema) and 1 (the hook stats' first
/// database, which held only their tables).
pub fn stamp() -> i64 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in schema().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    i64::from(hash % (i32::MAX as u32 - 2)) + 2
}

/// The stamp the first hook-stats database carried.
pub const HOOK_ONLY_STAMP: i64 = 1;

fn configure(conn: &Connection, path: &Path) -> Result<()> {
    let what = || format!("configuring {}", path.display());
    conn.busy_timeout(BUSY).context(what)?;
    conn.pragma_update(None, "foreign_keys", true)
        .context(what)?;
    conn.pragma_update(None, "synchronous", "FULL")
        .context(what)
}

/// The stamp a file holds: 0 for none.
pub fn stamp_of(conn: &Connection, path: &Path) -> Result<i64> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .context(|| format!("reading {}", path.display()))
}

/// Refuses a stamp that is not this schema's, naming both.
fn require_stamp(found: i64, path: &Path) -> Result<()> {
    if found != stamp() {
        fail!(
            "{} was made by another builds schema (it carries {found}, this builds' schema is {}); `builds install` moves a store of an older one",
            path.display(),
            stamp()
        );
    }
    Ok(())
}

fn has_tables(conn: &Connection, path: &Path) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .context(|| format!("reading {}", path.display()))?;
    Ok(count > 0)
}

/// How a writer may find the file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Make {
    /// A file that is not there is made, with the whole schema.
    Allowed,
    /// A file that is not there is an error.
    Never,
}

/// The database at <path> for writing, with the schema's stamp. The folder
/// must exist; this never makes it. A new file gets the whole schema and the
/// stamp in its first transaction, in WAL mode.
pub fn open(path: &Path, make: Make) -> Result<Connection> {
    let flags = match make {
        Make::Allowed => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        Make::Never => OpenFlags::SQLITE_OPEN_READ_WRITE,
    };
    let mut conn = Connection::open_with_flags(path, flags)
        .context(|| format!("opening {}", path.display()))?;
    configure(&conn, path)?;
    let found = stamp_of(&conn, path)?;
    if found == 0 {
        if has_tables(&conn, path)? {
            fail!(
                "{} holds tables and no schema stamp; it is no builds.sqlite of this builds",
                path.display()
            );
        }
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
            .context(|| format!("configuring {}", path.display()))?;
        let what = || format!("making the schema in {}", path.display());
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context(what)?;
        // Another opener may have made it while this one waited.
        if stamp_of(&tx, path)? == 0 {
            tx.execute_batch(&schema()).context(what)?;
            tx.pragma_update(None, "user_version", stamp())
                .context(what)?;
        }
        tx.commit().context(what)?;
        return Ok(conn);
    }
    require_stamp(found, path)?;
    Ok(conn)
}

/// The database at <path> for reading, read-only; None when there is no file
/// or no schema in it yet.
pub fn open_reader(path: &Path) -> Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context(|| format!("opening {}", path.display()))?;
    conn.busy_timeout(BUSY)
        .context(|| format!("configuring {}", path.display()))?;
    let found = stamp_of(&conn, path)?;
    if found == 0 {
        return Ok(None);
    }
    require_stamp(found, path)?;
    Ok(Some(conn))
}

/// The database at <path> for the move: a new file, or one holding only the
/// hook stats' tables under their first stamp. Neither has been given the
/// lists' tables yet; the caller does that and stamps it in one transaction.
pub fn open_for_move(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .context(|| format!("opening {}", path.display()))?;
    configure(&conn, path)?;
    let found = stamp_of(&conn, path)?;
    if found == 0 && has_tables(&conn, path)? {
        fail!(
            "{} holds tables and no schema stamp; it is no builds.sqlite of this builds",
            path.display()
        );
    }
    if found != 0 && found != HOOK_ONLY_STAMP && found != stamp() {
        require_stamp(found, path)?;
    }
    // A hook-stats database made before the lists were in it is in the default
    // mode; the lists' writers and readers share it, so it moves to WAL here.
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
        .context(|| format!("configuring {}", path.display()))?;
    Ok(conn)
}

/// A new in-memory database with the whole schema: the same pragmas, nothing
/// on disk. Tests build their lists in one, and the move rehearses a store in
/// one before it changes anything.
pub fn memory() -> Connection {
    let conn = Connection::open_in_memory().expect("an in-memory database opens");
    conn.pragma_update(None, "foreign_keys", true)
        .expect("pragma");
    conn.execute_batch(&schema()).expect("the schema applies");
    conn.pragma_update(None, "user_version", stamp())
        .expect("pragma");
    conn
}

/// The schema as the database holds it: every table and index, by name, with
/// its SQL.
pub fn schema_rows(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut statement = conn
        .prepare("SELECT type, name, COALESCE(sql, '') FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")
        .context(|| "reading the schema".to_string())?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .context(|| "reading the schema".to_string())?;
    rows.collect::<std::result::Result<_, _>>()
        .context(|| "reading the schema".to_string())
}

/// Names a refused or failed statement on the lists' database.
pub trait Sql<T> {
    fn sql(self) -> Result<T>;
}

impl<T> Sql<T> for rusqlite::Result<T> {
    fn sql(self) -> Result<T> {
        self.map_err(|error| {
            crate::fail::Fail::Message(format!("builds.sqlite refused the write: {error}"))
        })
    }
}

/// Writes one row of <table>, which must be one of <owned>, with the values
/// given and every constraint holding: the planting functions of the owners
/// that need a row a constraint refuses.
#[cfg(test)]
pub fn plant_row(
    db: &Connection,
    owned: &[&str],
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    assert!(
        owned.contains(&table),
        "{table} is not one of this owner's tables ({owned:?})"
    );
    let names: Vec<&str> = columns.iter().map(|(name, _)| *name).collect();
    let marks: Vec<String> = (1..=columns.len()).map(|n| format!("?{n}")).collect();
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        names.join(", "),
        marks.join(", ")
    );
    db.execute(
        &sql,
        rusqlite::params_from_iter(columns.iter().map(|(_, value)| value)),
    )
}

/// The names of the constraints a part of the schema gives, in its text.
#[cfg(test)]
pub fn constraint_names(sql: &str) -> Vec<String> {
    let words: Vec<&str> = sql
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    let mut names = Vec::new();
    for pair in words.windows(2) {
        if pair[0] == "CONSTRAINT" {
            names.push(pair[1].to_string());
        }
    }
    names
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
