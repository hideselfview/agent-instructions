//! Moves from an older shape of what the tool stores, done in place by
//! `builds install`.
//!
//! The first is the move from a lane's one app checkout to a set of them, done in place by
//! `builds install`: each repo's `app-checkout` file becomes `app-checkouts`
//! and its `app.lock` becomes `app.locks/<checkout>` (a hard link to the same
//! file, so the new path and the old are one lock), likewise for the android
//! lane's `app-android-*`. The queue keeps its folder and format.
//!
//! A `builds` from before the move reads the old names, and a build running
//! under it holds the old lock or waits in the old queue. The move does not
//! wait for those builds and does not interrupt them: they and the new `builds`
//! share the lock (one file under two names) and the queue, so they take turns
//! as before. The old names are removed once nothing uses them, which is the
//! first move run after the builds end (`builds install` runs it each time).

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::db::{self, DATABASE, Sql};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::queue::is_held;

/// Moves every repo's old-layout lane files to the new layout; does nothing
/// for a lane already moved.
pub fn migrate_app_files(places: &Places) -> Result<()> {
    let repos = match std::fs::read_dir(&places.root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", places.root.display()),
    };
    for repo in repos {
        let repo = repo.context(|| format!("listing {}", places.root.display()))?;
        let name = repo.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !repo.path().is_dir() {
            continue;
        }
        for lane in Lane::ALL {
            migrate_lane(places, &name, lane)?;
        }
    }
    Ok(())
}

fn migrate_lane(places: &Places, repo: &str, lane: Lane) -> Result<()> {
    let dir = places.root.join(repo);
    let old_file = dir.join(lane.legacy_checkout_file_name());
    let old_lock = dir.join(lane.legacy_lock_file_name());
    if !old_file.exists() && !old_lock.exists() {
        return Ok(());
    }
    if old_file.is_file() {
        let text = std::fs::read_to_string(&old_file)
            .context(|| format!("reading {}", old_file.display()))?;
        let checkout = text.trim().to_string();
        if checkout.is_empty() || checkout.contains('/') {
            fail!("{} names no side worktree ({text:?})", old_file.display());
        }
        // The old lock must exist before it is linked: a build from before
        // may be waiting to create it, and would make a second file.
        if !old_lock.exists() {
            crate::queue::open_with(&old_lock, libc::O_CREAT | libc::O_CLOEXEC)
                .map(|fd| unsafe { libc::close(fd) })
                .context(|| format!("creating {}", old_lock.display()))?;
        }
        let locks = places.app_locks_dir(repo, lane);
        std::fs::create_dir_all(&locks).context(|| format!("creating {}", locks.display()))?;
        let new_lock = locks.join(&checkout);
        if new_lock.exists() {
            let (old, new) = (std::fs::metadata(&old_lock), std::fs::metadata(&new_lock));
            let same = matches!((old, new), (Ok(old), Ok(new)) if old.ino() == new.ino() && old.dev() == new.dev());
            if !same {
                fail!(
                    "{} and {} are different files; remove the old one",
                    old_lock.display(),
                    new_lock.display()
                );
            }
        } else {
            std::fs::hard_link(&old_lock, &new_lock)
                .context(|| format!("linking {} to {}", new_lock.display(), old_lock.display()))?;
        }
        if !places.app_checkouts_file(repo, lane).exists() {
            places.write_app_checkouts(repo, lane, &[checkout])?;
        }
    }
    let queue = dir.join(lane.queue_dir_name());
    if (old_lock.exists() && is_held(&old_lock)?) || waiting_in(&queue)? {
        crate::text::note(format!(
            "{repo}'s {} lane has builds from before the move running or waiting; its old file names go the next time install runs with none",
            lane.word()
        ));
        return Ok(());
    }
    for old in [&old_lock, &old_file] {
        match std::fs::remove_file(old) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("removing {}: {error}", old.display())
            }
            _ => {}
        }
    }
    crate::text::note(format!(
        "moved {repo}'s {} app checkout to {}",
        lane.word(),
        places.app_checkouts_file(repo, lane).display()
    ));
    Ok(())
}

/// Does a process hold a ticket in the queue folder <queue>?
fn waiting_in(queue: &Path) -> Result<bool> {
    let entries = match std::fs::read_dir(queue) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => fail!("listing {}: {error}", queue.display()),
    };
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", queue.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with("order") {
            continue;
        }
        if is_held(&entry.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The stamp of the schema before the `design_deleted` table was added.
const BEFORE_DESIGN_DELETED_STAMP: i64 = 155_110_266;
/// The stamp of the schema before `hook_run_checks` kept a time only for a
/// passed check.
const BEFORE_CHECK_TIMES_STAMP: i64 = 213_607_842;
/// The stamp of the schema before the `design_tip` table was added.
const BEFORE_DESIGN_TIP_STAMP: i64 = 915_872_331;

/// The stamp of the schema before the app-build timing tables were added.
const BEFORE_APP_TIMING_STAMP: i64 = 431_659_017;

/// The stamp of the schema before the design sessions' table was added.
const BEFORE_DESIGN_SESSION_STAMP: i64 = 687_463_914;

/// The stamp of the schema before the shot findings' table was added.
const BEFORE_SHOT_FINDINGS_STAMP: i64 = 813_221_670;

/// The stamp of the schema before the review's tables were added.
const BEFORE_REVIEW_STAMP: i64 = 2_079_869_619;

/// The stamp of the schema before the cargo runs' table was added.
const BEFORE_CARGO_RUNS_STAMP: i64 = 95_960_216;

/// The stamp of the schema before the design fixes' table was added.
const BEFORE_DESIGN_FIX_STAMP: i64 = 187_372_354;

/// The stamp of the schema before reviews ran before landing: a review
/// landing closed when a review passed, and no run kept its commits or a
/// finding its fix.
const BEFORE_LANDING_REVIEW_STAMP: i64 = 548_041_900;

/// The stamp of the schema before each review call kept its prompt and cost.
const BEFORE_SCORED_CALLS_STAMP: i64 = 434_532_060;

/// The stamp of the schema before questions and items took tags.
const BEFORE_TAGS_STAMP: i64 = 395_623_277;

/// The stamp of the schema before builds dropped the cargo runs' table, which
/// nothing read.
const BEFORE_CARGO_RUNS_REMOVED_STAMP: i64 = 989_967_553;

/// The table as design.sql writes it.
const DESIGN_DELETED: &str = "CREATE TABLE design_deleted (
    design_id INTEGER NOT NULL REFERENCES design (id) ON DELETE CASCADE,
    path TEXT NOT NULL CONSTRAINT design_deleted_path_is_a_board_file CHECK (path NOT GLOB '*#*' AND path NOT GLOB '*/'),
    PRIMARY KEY (design_id, path)
) STRICT, WITHOUT ROWID";

/// Moves each project's `builds.sqlite` to today's schema: a store stamped as
/// the schema before the `design_deleted` table gets that table, a store
/// stamped as either older schema gets a `hook_run_checks` that keeps a time
/// only for a passed check (a failed check's time is dropped), and every store
/// gets the empty `design_tip` table (a design change has no last commit until
/// it is amended), every store older than the app-build timings gets their
/// tables, and every store older than today's gets the empty
/// `design_session` table (a design change has no session to tell until it is
/// recorded or amended), and every store older than today's gets the empty
/// `shot_findings` table (no run's findings are kept yet), every store gets
/// the review's empty tables, and every store gets the empty `design_fix`
/// table (no change is a fix yet), and every store whose review tables are
/// older than today's gets `review_landing` with `landed_at` and `landed_run` in place of `closed_at` (a landing whose review
/// passed landed with its newest review run, since that review ran after the
/// push) and the empty `review_commit` and `review_fix` tables, and every store
/// whose review tables are older than today's gets `review_call` with each
/// call's prompt, read from the system prompt it was sent, and no cost, and
/// every store gets the empty `record_tag` table (no question or item carries
/// a tag yet), and the move drops the `cargo_runs` table, which nothing read,
/// and its rows from every store. The moves run in one transaction, the store
/// is stamped as today's schema, and it must then equal a fresh database's schema or the transaction rolls back. A
/// store already at today's stamp, or with no database, is left alone; any
/// other stamp is refused. Delete this once it has run.
pub fn migrate_stores(places: &Places) -> Result<()> {
    let entries = match std::fs::read_dir(&places.lists) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", places.lists.display()),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", places.lists.display()))?;
        if entry.path().is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    for dir in dirs {
        move_store(&dir.join(DATABASE))?;
    }
    Ok(())
}

/// The statements that rebuild `hook_run_checks` as hook_stats.sql writes it,
/// taking the table and its index from today's schema text so the result
/// equals a fresh database's, and keeping a time only for a passed check.
fn check_times() -> String {
    let schema = db::schema();
    let slice = |from: &str, end: &str| {
        let start = schema.find(from).expect("the schema has it");
        let stop = start + schema[start..].find(end).expect("it ends") + end.len();
        schema[start..stop].to_string()
    };
    let table = slice(
        "CREATE TABLE IF NOT EXISTS hook_run_checks",
        "WITHOUT ROWID;",
    );
    let index = slice("CREATE INDEX IF NOT EXISTS idx_hook_run_checks_name", ";");
    format!(
        "ALTER TABLE hook_run_checks RENAME TO hook_run_checks_old;
{table}
INSERT INTO hook_run_checks (run_id, position, name, kind, outcome, files, crates, ms)
    SELECT run_id, position, name, kind, outcome, files, crates, CASE WHEN outcome = 'passed' THEN ms END
    FROM hook_run_checks_old;
DROP TABLE hook_run_checks_old;
{index}"
    )
}

/// The statement that makes `design_tip` as design.sql writes it, taken from
/// today's schema text so the result equals a fresh database's.
fn design_tip() -> String {
    let schema = db::schema();
    let start = schema
        .find("CREATE TABLE design_tip")
        .expect("the schema has it");
    let end = "STRICT;";
    let stop = start + schema[start..].find(end).expect("it ends") + end.len();
    schema[start..stop].to_string()
}

/// The statement that makes `design_fix` as design.sql writes it, taken from
/// today's schema text so the result equals a fresh database's.
fn design_fix() -> String {
    let schema = db::schema();
    let start = schema
        .find("CREATE TABLE design_fix")
        .expect("the schema has it");
    let end = "STRICT;";
    let stop = start + schema[start..].find(end).expect("it ends") + end.len();
    schema[start..stop].to_string()
}

fn move_store(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let mut conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context(|| format!("opening {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(i32::MAX as u64))
        .sql()?;
    let found = db::stamp_of(&conn, path)?;
    if found == db::stamp() || found == 0 {
        return Ok(());
    }
    if ![
        BEFORE_DESIGN_DELETED_STAMP,
        BEFORE_CHECK_TIMES_STAMP,
        BEFORE_DESIGN_TIP_STAMP,
        BEFORE_APP_TIMING_STAMP,
        BEFORE_DESIGN_SESSION_STAMP,
        BEFORE_SHOT_FINDINGS_STAMP,
        BEFORE_REVIEW_STAMP,
        BEFORE_CARGO_RUNS_STAMP,
        BEFORE_DESIGN_FIX_STAMP,
        BEFORE_LANDING_REVIEW_STAMP,
        BEFORE_SCORED_CALLS_STAMP,
        BEFORE_TAGS_STAMP,
        BEFORE_CARGO_RUNS_REMOVED_STAMP,
    ]
    .contains(&found)
    {
        fail!(
            "{} carries stamp {found}, which this builds does not know how to move (it knows {BEFORE_DESIGN_DELETED_STAMP}, {BEFORE_CHECK_TIMES_STAMP}, {BEFORE_DESIGN_TIP_STAMP}, {BEFORE_APP_TIMING_STAMP}, {BEFORE_DESIGN_SESSION_STAMP}, {BEFORE_SHOT_FINDINGS_STAMP}, {BEFORE_REVIEW_STAMP}, {BEFORE_CARGO_RUNS_STAMP}, {BEFORE_DESIGN_FIX_STAMP}, {BEFORE_LANDING_REVIEW_STAMP}, {BEFORE_SCORED_CALLS_STAMP}, {BEFORE_TAGS_STAMP} and {BEFORE_CARGO_RUNS_REMOVED_STAMP}, and today's {})",
            path.display(),
            db::stamp()
        );
    }
    // Off for the moves, so a table made again keeps the references to it as
    // they are written; every reference is checked before the commit.
    conn.pragma_update(None, "foreign_keys", false).sql()?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .sql()?;
    // A store from before reviews ran before landing, or after, has every
    // other part.
    if ![
        BEFORE_LANDING_REVIEW_STAMP,
        BEFORE_SCORED_CALLS_STAMP,
        BEFORE_TAGS_STAMP,
        BEFORE_CARGO_RUNS_REMOVED_STAMP,
    ]
    .contains(&found)
    {
        if found == BEFORE_DESIGN_DELETED_STAMP {
            tx.execute_batch(DESIGN_DELETED).sql()?;
        }
        let has_tip_and_times = [
            BEFORE_APP_TIMING_STAMP,
            BEFORE_DESIGN_SESSION_STAMP,
            BEFORE_SHOT_FINDINGS_STAMP,
            BEFORE_REVIEW_STAMP,
            BEFORE_CARGO_RUNS_STAMP,
            BEFORE_DESIGN_FIX_STAMP,
        ]
        .contains(&found);
        if found != BEFORE_DESIGN_TIP_STAMP && !has_tip_and_times {
            tx.execute_batch(&check_times()).sql()?;
        }
        if !has_tip_and_times {
            tx.execute_batch(&design_tip()).sql()?;
        }
        if ![
            BEFORE_DESIGN_SESSION_STAMP,
            BEFORE_SHOT_FINDINGS_STAMP,
            BEFORE_REVIEW_STAMP,
            BEFORE_CARGO_RUNS_STAMP,
            BEFORE_DESIGN_FIX_STAMP,
        ]
        .contains(&found)
        {
            tx.execute_batch(db::APP_TIMING_PART).sql()?;
        }
        // Every part is `IF NOT EXISTS`, so a store that has one keeps it.
        tx.execute_batch(db::DESIGN_SESSION_PART).sql()?;
        tx.execute_batch(db::SHOT_FINDINGS_PART).sql()?;
        // The review's tables are plain `CREATE TABLE`s: a store from just before
        // the cargo runs' table, or after it, already has them.
        if found != BEFORE_CARGO_RUNS_STAMP && found != BEFORE_DESIGN_FIX_STAMP {
            tx.execute_batch(db::REVIEW_PART).sql()?;
        }
        tx.execute_batch(&design_fix()).sql()?;
    }
    // A store that has the review's tables has them as they were before
    // reviews ran before landing; one that got them above has today's.
    if [
        BEFORE_CARGO_RUNS_STAMP,
        BEFORE_DESIGN_FIX_STAMP,
        BEFORE_LANDING_REVIEW_STAMP,
    ]
    .contains(&found)
    {
        crate::review::record::move_to_landing_review(&tx)?;
    }
    // A store that has the review's tables has calls without their prompt
    // and cost; one that got them above has today's.
    if [
        BEFORE_CARGO_RUNS_STAMP,
        BEFORE_DESIGN_FIX_STAMP,
        BEFORE_LANDING_REVIEW_STAMP,
        BEFORE_SCORED_CALLS_STAMP,
    ]
    .contains(&found)
    {
        crate::review::record::move_to_scored_calls(&tx)?;
    }
    // Of the stores this moves, only one stamped just before builds dropped the
    // cargo runs' table already has the tags' table.
    if found != BEFORE_CARGO_RUNS_REMOVED_STAMP {
        tx.execute_batch(db::TAG_PART).sql()?;
    }
    // Stores stamped after builds added the cargo runs' table still have it;
    // dropping the table also drops its index.
    tx.execute_batch("DROP TABLE IF EXISTS cargo_runs;").sql()?;
    tx.pragma_update(None, "user_version", db::stamp()).sql()?;
    let broken: Option<String> = tx
        .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
        .optional()
        .sql()?;
    if let Some(table) = broken {
        fail!(
            "{} holds a row of {table} that points at no row after the move; it is left as it was",
            path.display()
        );
    }
    if db::schema_rows(&tx)? != db::schema_rows(&db::memory())? {
        fail!(
            "{} holds a schema that is not the one this builds makes; it is left as it was",
            path.display()
        );
    }
    tx.commit().sql()?;
    crate::text::note(format!("moved {} to today's schema", path.display()));
    Ok(())
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;
