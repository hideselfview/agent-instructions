//! `builds.sqlite`, the one database in each project's store folder: the
//! lists (every table of `lists/*.sql`), the hook-run stats (`hook_stats.sql`
//! and `hook_check_starts.sql`), the app-build timings (`app_timing.sql`), the
//! design changes' recording sessions (`lists/design_session.sql`), the shot
//! findings, the review's tables, the `builds cargo` runs
//! (`cargo_run.sql`), the spec checks (`lists/spec_check.sql`), the agents'
//! acknowledgements (`ack/record.sql`). This module does all opening:
//! the pragmas, the schema made whole in the first transaction of a new file,
//! and the stamp.
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
const LISTS_PARTS: [&str; 11] = [
    include_str!("lists/project.sql"),
    include_str!("lists/area.sql"),
    include_str!("lists/record.sql"),
    TAG_PART,
    include_str!("lists/question.sql"),
    include_str!("lists/decision.sql"),
    include_str!("lists/item.sql"),
    include_str!("lists/running.sql"),
    include_str!("lists/landing.sql"),
    include_str!("lists/design.sql"),
    include_str!("lists/store.sql"),
];
const HOOK_PART: &str = include_str!("hook_stats.sql");

/// The tags of questions and items (lists/tag.sql), read after the records.
pub const TAG_PART: &str = include_str!("lists/tag.sql");
/// The app-build timing tables (app_timing.sql), the last part of the schema.
pub const APP_TIMING_PART: &str = include_str!("app_timing.sql");

/// The design changes' recording sessions (lists/design_session.sql), the last
/// part of the schema.
pub const DESIGN_SESSION_PART: &str = include_str!("lists/design_session.sql");

/// A busy store waits as `flock` did: until the other write ends. SQLite
/// takes the wait as milliseconds in an `i32`.
const BUSY: Duration = Duration::from_millis(i32::MAX as u64);

/// The stored findings of `--shots` runs (shot_findings.sql), the last part of
/// the schema.
pub const SHOT_FINDINGS_PART: &str = include_str!("shot_findings.sql");

/// The review's tables (review/record.sql).
pub const REVIEW_PART: &str = include_str!("review/record.sql");

/// This Mac's load when each hook check started (hook_check_starts.sql).
pub const HOOK_CHECK_STARTS_PART: &str = include_str!("hook_check_starts.sql");

/// The record of the `builds cargo` runs hook checks started (cargo_run.sql),
/// the last part of the schema.
pub const CARGO_RUNS_PART: &str = include_str!("cargo_run.sql");

/// Where open items keep a worktree with no agent (lists/kept_work.sql); the
/// schema holds it after the cargo runs' part.
pub const KEPT_WORK_PART: &str = include_str!("lists/kept_work.sql");

/// The work an item held when the person dropped it (lists/dropped.sql); the
/// schema holds it after the kept work's part.
pub const DROPPED_PART: &str = include_str!("lists/dropped.sql");

/// The checks of briefs against the product spec (lists/spec_check.sql); the
/// schema holds it after the dropped items' part.
pub const SPEC_CHECK_PART: &str = include_str!("lists/spec_check.sql");

/// The agents' acknowledgements of the text builds asks them to read
/// (ack/record.sql); the schema holds it after the spec checks' part.
pub const ACK_PART: &str = include_str!("ack/record.sql");

/// The whole schema: the lists' tables, then the hook stats', then the
/// app-build timings', then the design changes' recording sessions', then the
/// stored shot findings, then the review's, then the hook checks' starting
/// loads, then the cargo runs', then the kept work's, then the dropped
/// items' work, then the spec checks', then the acknowledgements'.
pub fn schema() -> String {
    format!(
        "{}\n{REVIEW_PART}\n{HOOK_CHECK_STARTS_PART}\n{CARGO_RUNS_PART}\n{KEPT_WORK_PART}\n{DROPPED_PART}\n{SPEC_CHECK_PART}\n{ACK_PART}",
        schema_before_review()
    )
}

/// The schema as it was before the review's tables joined it.
pub fn schema_before_review() -> String {
    format!("{}\n{SHOT_FINDINGS_PART}", schema_before_shot_findings())
}

/// The schema as it was before the shot findings' table joined it.
pub fn schema_before_shot_findings() -> String {
    format!("{}\n{DESIGN_SESSION_PART}", schema_before_design_session())
}

/// The schema as it was before the design sessions' table joined it.
pub fn schema_before_design_session() -> String {
    format!("{}\n{APP_TIMING_PART}", schema_before_app_timing())
}

/// The schema as it was before the app-build timing tables joined it.
pub fn schema_before_app_timing() -> String {
    format!("{}\n{HOOK_PART}", LISTS_PARTS.join("\n"))
}

/// The statement of <schema> that starts with <from>, through the first <end>
/// after it. A move that makes a table again takes the table's statements
/// from the schema text this way, so the table equals a fresh database's.
pub fn statement<'a>(schema: &'a str, from: &str, end: &str) -> &'a str {
    let Some(start) = schema.find(from) else {
        panic!("the schema has no statement that starts with {from:?}");
    };
    let Some(length) = schema[start..].find(end) else {
        panic!("the statement that starts with {from:?} has no {end:?} after it");
    };
    &schema[start..start + length + end.len()]
}

/// Makes table <table> again as <create> writes it, for a store move: renames
/// the old table to `<table>_old`, runs <create>, lets <copy> copy the old
/// rows from `<table>_old` into the new table, drops the old table and runs
/// <after> (the table's indexes and any table the move adds beside it).
/// migrate.rs runs a move with the foreign keys off, and SQLite renames the
/// old table under `legacy_alter_table`, so the tables that point at <table>
/// keep naming it.
pub fn remake_table(
    db: &Connection,
    table: &str,
    create: &str,
    copy: impl FnOnce(&Connection) -> Result<()>,
    after: &[&str],
) -> Result<()> {
    db.execute_batch(&format!(
        "PRAGMA legacy_alter_table = ON;
ALTER TABLE {table} RENAME TO {table}_old;
{create}"
    ))
    .sql()?;
    copy(db)?;
    db.execute_batch(&format!(
        "DROP TABLE {table}_old;
{}
PRAGMA legacy_alter_table = OFF;",
        after.join("\n")
    ))
    .sql()
}

/// The stamp of the schema text: its FNV-1a hash (`text::fnv1a`), folded into `user_version`'s
/// positive range and kept off 0 (no schema) and 1 (which a hook-stats-only
/// database once carried).
pub fn stamp() -> i64 {
    stamp_of_text(&schema())
}

/// The stamp of a schema whose text is <text>. `stamp` takes it of today's
/// schema, and the tests take it of each older schema the moves know.
pub fn stamp_of_text(text: &str) -> i64 {
    i64::from(crate::text::fnv1a(text) % (i32::MAX as u32 - 2)) + 2
}

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

/// The stamp the database at <path> holds, read without the stamp check and
/// without a use of the stores: 0 for none. `builds install` reads it to
/// learn whether it moves the store.
pub fn stamp_at(path: &Path) -> Result<i64> {
    stamp_of(&connect_read_only(path)?, path)
}

/// Refuses a stamp that is not this schema's, naming both.
fn require_stamp(found: i64, path: &Path) -> Result<()> {
    if found != stamp() && crate::install_gate::install_ran()? {
        fail!(
            "builds install ran while this command ran, and moved {} to the installed program's schema; run the command again",
            path.display()
        );
    }
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

/// A connection to a store, and the use of the stores it takes while it is
/// open (install_gate.rs).
pub struct Held {
    // Rust drops fields in the order they are declared, so `conn` closes
    // before `_use` ends the use.
    conn: Connection,
    _use: Option<crate::install_gate::ConnectionUse>,
}

impl std::ops::Deref for Held {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.conn.fmt(f)
    }
}

impl std::ops::DerefMut for Held {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

/// The database at <path> for writing, with the schema's stamp. The folder
/// must exist; this never makes it. A new file gets the whole schema and the
/// stamp in its first transaction, in WAL mode.
pub fn open(path: &Path, make: Make) -> Result<Held> {
    let taken = crate::install_gate::for_connection()?;
    let conn = open_connection(path, make)?;
    Ok(Held { conn, _use: taken })
}

fn open_connection(path: &Path, make: Make) -> Result<Connection> {
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
pub fn open_reader(path: &Path) -> Result<Option<Held>> {
    if !path.exists() {
        return Ok(None);
    }
    let taken = crate::install_gate::for_connection()?;
    Ok(open_read_only(path)?.map(|conn| Held { conn, _use: taken }))
}

/// A read-only connection to the database at <path> that waits while
/// another connection writes, with no stamp check.
fn connect_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context(|| format!("opening {}", path.display()))?;
    conn.busy_timeout(BUSY)
        .context(|| format!("configuring {}", path.display()))?;
    Ok(conn)
}

fn open_read_only(path: &Path) -> Result<Option<Connection>> {
    let conn = connect_read_only(path)?;
    let found = stamp_of(&conn, path)?;
    if found == 0 {
        return Ok(None);
    }
    require_stamp(found, path)?;
    Ok(Some(conn))
}

/// A new in-memory database with the whole schema: the same pragmas, nothing
/// on disk. Tests build their lists in one.
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
