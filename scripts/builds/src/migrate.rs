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

use crate::db::{self, Sql};
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
            places.write_app_checkouts(
                repo,
                lane,
                &[crate::lane::AppCheckout::by_person(&checkout)],
            )?;
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

/// The stamp of the schema before a condition named who ends it, the first
/// schema whose stamp is the hash of its shape.
const BEFORE_CONDITION_ENDERS_STAMP: i64 = 1_173_303_068;

/// The stamp of the schema before builds recorded the moments an item changed
/// state or gained or lost a wait.
const BEFORE_ITEM_EVENT_STAMP: i64 = 2_055_393_844;

/// The stamp of the schema before builds recorded each `builds land` run.
const BEFORE_LAND_RUN_STAMP: i64 = 273_609_669;

/// The stamp of the schema before builds recorded when each reviewer process
/// began and how long it ran.
const BEFORE_EXCHANGE_TIME_STAMP: i64 = 94_915_310;

/// The stamp of the schema before builds recorded the hosts' load and memory
/// over time.
const BEFORE_HOST_SAMPLE_STAMP: i64 = 1_996_518_111;

/// The stamp of the schema before builds recorded the crates each `builds
/// cargo` run named.
const BEFORE_CARGO_RUN_CRATE_STAMP: i64 = 277_344_443;

/// The stamp of the schema before the judge's tables were added.
const BEFORE_JUDGE_STAMP: i64 = 836_393_942;

/// The stamp of the schema before builds found a review's landing by its repo
/// and key.
const BEFORE_LANDING_REPO_STAMP: i64 = 34_147_721;

/// The stamp of the schema before builds recorded each change of a worktree
/// card's dashboard state.
const BEFORE_CARD_STATE_STAMP: i64 = 21_217_016;

/// The stamp of the schema before builds recorded how each `builds cargo` run
/// ended.
const BEFORE_CARGO_RUN_OUTCOME_STAMP: i64 = 949_736_987;

/// The stamp of the schema before builds recorded the commit each cargo run
/// ran on and each app build built.
const BEFORE_STEP_COMMITS_STAMP: i64 = 96_262_301;

/// The stamp of the schema before the review's texts sat in a table with row
/// ids.
const BEFORE_REVIEW_TEXT_ROWID_STAMP: i64 = 116_876_965;

/// The stamp of the schema before builds recorded what each failed hook check
/// said.
const BEFORE_HOOK_CHECK_FAILURE_STAMP: i64 = 321_616_823;

/// The stamp of the schema before builds recorded what each failed app build
/// step said.
const BEFORE_APP_BUILD_STEP_FAILURE_STAMP: i64 = 1_750_228_100;

/// The stamp of the schema before builds recorded which findings it told the
/// judge of.
const BEFORE_JUDGE_TOLD_STAMP: i64 = 1_477_190_891;

/// The stamp of the schema before the circles of a review board that surround
/// a box.
const BEFORE_DESIGN_CIRCLE_BOX_STAMP: i64 = 1_621_364_442;

/// The stamp of the schema before a design place could name no app work yet.
const BEFORE_DESIGN_PLACE_NAMING_STAMP: i64 = 889_221_987;

/// The stamp of the schema before the variants of a design change.
const BEFORE_DESIGN_VARIANTS_STAMP: i64 = 66_978_370;

/// The stamp of the schema before the branches over the review's cap.
const BEFORE_OVER_CAP_STAMP: i64 = 1_558_212_582;

/// The stamp of the schema before the review section kept only its heading.
const BEFORE_REVIEW_SECTION_HEADING_ONLY_STAMP: i64 = 2_044_746_774;

/// The stamp of the schema before the variants of a design change left it.
const BEFORE_DESIGN_VARIANTS_LEFT_STAMP: i64 = 294_983_269;

/// The stamp of the schema before a landing's steps took the wait for the
/// turn of a landing that changes the apps and the app build of the result.
const BEFORE_LAND_APP_STEPS_STAMP: i64 = 1_952_688_938;

/// The stamp of the schema before a review board said whether the review
/// page shows its now.
const BEFORE_REVIEW_BOARD_SHOW_NOW_STAMP: i64 = 716_196_970;

/// The stamp of the schema before an app build run could end stopped.
const BEFORE_APP_BUILD_STOPPED_STAMP: i64 = 978_764_755;

/// The stamp of the schema before builds kept the shots runs on the build
/// volume (shots_cache.rs) instead of in the store.
const BEFORE_SHOTS_ON_VOLUME_STAMP: i64 = 1_758_641_369;

/// The stamp of the schema before builds kept the review texts on the build
/// volume (review/texts.rs) instead of in the store.
const BEFORE_REVIEW_TEXTS_ON_VOLUME_STAMP: i64 = 1_364_416_336;

/// The stamp of the schema before the branches the coordinator marked as
/// mechanical.
const BEFORE_MARKED_MECHANICAL_STAMP: i64 = 332_045_652;

/// The stamp of today's schema (`db::STAMP`). A change to the schema changes
/// its stamp, and builds then fails to compile until the change gives it a
/// move from this schema, since every store carries this stamp: the change
/// adds the move, puts this stamp at the end of `STAMPS` under a name that
/// says what the schema lacked, and sets this to the new schema's stamp, which
/// the compiler's error names.
const SCHEMA_STAMP: i64 = 1_544_708_784;

// The compiler refuses a schema whose stamp is not SCHEMA_STAMP. A const
// assert could not print the new stamp, so the check compares two array
// lengths, and the compiler's error names both: the array's expected size is
// SCHEMA_STAMP, and the size it found is the new schema's stamp.
const _: [(); SCHEMA_STAMP as usize] = [(); db::STAMP as usize];

/// Every stamp a store can carry that `move_store` moves, oldest schema first.
/// A new move adds the stamp of the schema before it at the end.
const STAMPS: [i64; 27] = [
    BEFORE_CONDITION_ENDERS_STAMP,
    BEFORE_ITEM_EVENT_STAMP,
    BEFORE_LAND_RUN_STAMP,
    BEFORE_EXCHANGE_TIME_STAMP,
    BEFORE_HOST_SAMPLE_STAMP,
    BEFORE_CARGO_RUN_CRATE_STAMP,
    BEFORE_JUDGE_STAMP,
    BEFORE_LANDING_REPO_STAMP,
    BEFORE_CARD_STATE_STAMP,
    BEFORE_CARGO_RUN_OUTCOME_STAMP,
    BEFORE_STEP_COMMITS_STAMP,
    BEFORE_REVIEW_TEXT_ROWID_STAMP,
    BEFORE_HOOK_CHECK_FAILURE_STAMP,
    BEFORE_APP_BUILD_STEP_FAILURE_STAMP,
    BEFORE_JUDGE_TOLD_STAMP,
    BEFORE_DESIGN_CIRCLE_BOX_STAMP,
    BEFORE_DESIGN_PLACE_NAMING_STAMP,
    BEFORE_DESIGN_VARIANTS_STAMP,
    BEFORE_OVER_CAP_STAMP,
    BEFORE_REVIEW_SECTION_HEADING_ONLY_STAMP,
    BEFORE_DESIGN_VARIANTS_LEFT_STAMP,
    BEFORE_LAND_APP_STEPS_STAMP,
    BEFORE_REVIEW_BOARD_SHOW_NOW_STAMP,
    BEFORE_APP_BUILD_STOPPED_STAMP,
    BEFORE_SHOTS_ON_VOLUME_STAMP,
    BEFORE_REVIEW_TEXTS_ON_VOLUME_STAMP,
    BEFORE_MARKED_MECHANICAL_STAMP,
];

/// Whether a store stamped <found> comes before the move that follows the
/// schema stamped <stamp>, so that move applies to it: <found> is <stamp> or
/// an older stamp of `STAMPS`.
fn before(found: i64, stamp: i64) -> bool {
    let at = |s: i64| {
        STAMPS
            .iter()
            .position(|known| *known == s)
            .expect("move_store moves only stamps that STAMPS holds")
    };
    at(found) <= at(stamp)
}

/// Moves each project's `builds.sqlite` to today's schema. Every store from
/// before a condition named who ends it gets the coordinator as the ender of
/// each condition (`lists::item::move_to_condition_enders`); every store from
/// before the item events gets their empty table with the mark of when the
/// store began to record them (<now>, the moment install moves it); every
/// older store gets the empty tables that joined the schema after its stamp:
/// the `builds land` runs, the reviewer processes' times, the hosts' samples,
/// the crates the cargo runs named and the judge; every store from before
/// builds found a review's landing by its repo and key gets the indexes that
/// find it so, one of which refuses two open landings of one change; and
/// every store from before builds kept the shots runs on the build volume
/// loses the shot findings' table, and once the move commits, the folder of
/// reports beside the database; every store from before builds kept the
/// review texts on the build volume writes them there, loses their table and
/// holds the hash of each reviewer's answer in place of the answer, and once
/// the move commits, the freed pages (a vacuum). The moves
/// run in one transaction, the
/// store is stamped as today's schema, and it must then equal a fresh
/// database's schema or the transaction rolls back. A store already at
/// today's stamp, or with no database, is left alone; any other stamp is
/// refused, and the refusal names it.
pub fn migrate_stores(places: &Places, now: i64) -> Result<()> {
    for store in crate::lists::store::database_paths(places)? {
        move_store(places, &store, now)?;
    }
    Ok(())
}

/// Whether every store already has this program's schema, so `migrate_stores`
/// moves none: true when one store at least has a schema and every store that
/// has one has this program's. The function skips a store with no schema yet.
/// When no store has a schema, the function cannot tell which schema the
/// installed program writes, so it returns false.
pub fn every_store_has_this_schema(places: &Places) -> Result<bool> {
    let mut any = false;
    for store in crate::lists::store::database_paths(places)? {
        if !store.is_file() {
            continue;
        }
        match db::stamp_at(&store)? {
            0 => {}
            found if found == db::stamp() => any = true,
            _ => return Ok(false),
        }
    }
    Ok(any)
}

/// Moves the store at <path> to today's schema, as `migrate_stores` moves
/// each project's store, at <now>; also the store an app build lane hands the
/// installed builds (handed_runs.rs).
pub fn move_store(places: &Places, path: &Path, now: i64) -> Result<()> {
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
    if !STAMPS.contains(&found) {
        let known: Vec<String> = STAMPS.iter().map(i64::to_string).collect();
        fail!(
            "{} carries stamp {found}, which this builds does not know how to move (it knows {}, and today's {})",
            path.display(),
            known.join(", "),
            db::stamp()
        );
    }
    // Off for the moves, so a table made again keeps the references to it as
    // they are written; every reference is checked before the commit.
    conn.pragma_update(None, "foreign_keys", false).sql()?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .sql()?;
    // Every store from before a condition named who ends it gets the
    // coordinator as the ender of each condition it holds.
    if before(found, BEFORE_CONDITION_ENDERS_STAMP) {
        crate::lists::item::move_to_condition_enders(&tx)?;
    }
    // Every store from before the item events gets their empty table, and the
    // mark that says it records them from now on.
    if before(found, BEFORE_ITEM_EVENT_STAMP) {
        tx.execute_batch(db::ITEM_EVENT_PART).sql()?;
        crate::lists::item_event::start_recording(&tx, now)?;
    }
    // Every store from before the landings' record gets their empty tables.
    if before(found, BEFORE_LAND_RUN_STAMP) {
        tx.execute_batch(db::LAND_RUN_PART).sql()?;
    }
    // Every store from before the reviewer processes' times gets their empty
    // table.
    if before(found, BEFORE_EXCHANGE_TIME_STAMP) {
        tx.execute_batch(db::EXCHANGE_TIME_PART).sql()?;
    }
    // Every store from before the hosts' samples gets their empty table.
    if before(found, BEFORE_HOST_SAMPLE_STAMP) {
        tx.execute_batch(db::HOST_SAMPLE_PART).sql()?;
    }
    // Every store from before the cargo runs' crates gets their empty table.
    if before(found, BEFORE_CARGO_RUN_CRATE_STAMP) {
        tx.execute_batch(db::CARGO_RUN_CRATE_PART).sql()?;
    }
    // Every store from before the judge gets its empty table: no session
    // judges yet.
    if before(found, BEFORE_JUDGE_STAMP) {
        tx.execute_batch(db::JUDGE_PART).sql()?;
    }
    // Every store from before builds found a review's landing by its repo and
    // key gets the indexes that find it so in place of the index on the key
    // alone. The unique index refuses a store with two open landings of one
    // change, and the move then rolls back.
    if before(found, BEFORE_LANDING_REPO_STAMP) {
        tx.execute_batch(&format!(
            "DROP INDEX review_landing_key;\n{}\n{}",
            db::statement(db::REVIEW_PART, "CREATE INDEX review_landing_change", ";"),
            db::statement(
                db::REVIEW_PART,
                "CREATE UNIQUE INDEX review_landing_open_once",
                ";"
            )
        ))
        .sql()?;
    }
    // Every store from before the cards' states gets their empty tables.
    if before(found, BEFORE_CARD_STATE_STAMP) {
        tx.execute_batch(db::CARD_STATE_PART).sql()?;
    }
    // Every store from before the cargo runs' outcomes gets their empty
    // tables: the runs it holds have none.
    if before(found, BEFORE_CARGO_RUN_OUTCOME_STAMP) {
        tx.execute_batch(db::CARGO_RUN_OUTCOME_PART).sql()?;
    }
    // Every store from before the steps' commits gets their empty tables.
    if before(found, BEFORE_STEP_COMMITS_STAMP) {
        tx.execute_batch(db::CARGO_RUN_COMMIT_PART).sql()?;
        tx.execute_batch(db::APP_BUILD_COMMIT_PART).sql()?;
    }
    // Every store from before the failed hook checks' words gets their empty
    // tables: the checks it holds have none.
    if before(found, BEFORE_HOOK_CHECK_FAILURE_STAMP) {
        tx.execute_batch(db::HOOK_CHECK_FAILURE_PART).sql()?;
    }
    // Every store from before the failed app build steps' words gets their
    // empty tables: the steps it holds have none.
    if before(found, BEFORE_APP_BUILD_STEP_FAILURE_STAMP) {
        tx.execute_batch(db::APP_BUILD_STEP_FAILURE_PART).sql()?;
    }
    // Every store from before builds recorded which findings it told the
    // judge of gets the empty table: it told the judge of none.
    if before(found, BEFORE_JUDGE_TOLD_STAMP) {
        tx.execute_batch(db::JUDGE_TOLD_PART).sql()?;
    }
    // Every store from before the circles around a box gets their empty
    // table: no review board has one yet.
    if before(found, BEFORE_DESIGN_CIRCLE_BOX_STAMP) {
        tx.execute_batch(db::DESIGN_CIRCLE_BOX_PART).sql()?;
    }
    // Every store from before a design place could name no app work yet gets
    // the place table again with the check that allows it; each place keeps
    // its app work.
    if before(found, BEFORE_DESIGN_PLACE_NAMING_STAMP) {
        crate::lists::design::allow_places_with_no_app_work(&tx)?;
    }
    // Every store from before the branches over the review's cap gets the
    // empty table: no branch has been over it yet.
    if before(found, BEFORE_OVER_CAP_STAMP) {
        tx.execute_batch(db::OVER_CAP_PART).sql()?;
    }
    // Every store from before the review section kept only its heading gets
    // the table again without the words; each change keeps its heading.
    if before(found, BEFORE_REVIEW_SECTION_HEADING_ONLY_STAMP) {
        crate::lists::design::drop_review_section_words(&tx)?;
    }
    // Every store whose schema held the variants of a design change loses
    // their tables, and with them the versions they kept: scratch boards in
    // the worktrees replace them, and nothing reads the variants.
    if !before(found, BEFORE_DESIGN_VARIANTS_STAMP)
        && before(found, BEFORE_DESIGN_VARIANTS_LEFT_STAMP)
    {
        tx.execute_batch("DROP TABLE design_variant;\nDROP TABLE design_variants;")
            .sql()?;
    }
    // Every store from before a landing's steps took the app wait and the
    // app build gets the steps' table again with the check that allows them;
    // each step keeps its row.
    if before(found, BEFORE_LAND_APP_STEPS_STAMP) {
        crate::land_run::allow_app_steps(&tx)?;
    }
    // Every store from before a review board said whether the page shows its
    // now gets the board table again with the column; each board keeps its
    // circles and shows its now, as every board's page did then.
    if before(found, BEFORE_REVIEW_BOARD_SHOW_NOW_STAMP) {
        crate::lists::design::add_review_board_show_now(&tx)?;
    }
    // Every store from before an app build run could end stopped gets the
    // runs' table again with how each run ended, and the steps' table again
    // with the check that takes a stopped step; each run and step keeps its
    // row.
    if before(found, BEFORE_APP_BUILD_STOPPED_STAMP) {
        crate::app_timing::record_stopped_runs(&tx)?;
    }
    // Every store from before builds kept the shots runs on the build volume
    // loses the findings' table; the folder of reports beside the database
    // goes once the move commits.
    if before(found, BEFORE_SHOTS_ON_VOLUME_STAMP) {
        tx.execute_batch("DROP TABLE shot_findings;").sql()?;
    }
    // Every store from before builds kept the review texts on the build
    // volume writes them there, and holds hashes in their place.
    if before(found, BEFORE_REVIEW_TEXTS_ON_VOLUME_STAMP) {
        let project = path
            .parent()
            .and_then(Path::file_name)
            .expect("a store's database is in its project's folder")
            .to_string_lossy();
        crate::review::record::move_texts_to_volume(
            &tx,
            &crate::review::texts::Texts::of(places, &project),
        )?;
    }
    // Every store from before the coordinator could mark a branch as
    // mechanical gets the empty table: none was marked yet.
    if before(found, BEFORE_MARKED_MECHANICAL_STAMP) {
        tx.execute_batch(db::MARKED_MECHANICAL_PART).sql()?;
    }
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
    if before(found, BEFORE_REVIEW_TEXTS_ON_VOLUME_STAMP) {
        // The dropped tables' pages stay in the file as free pages until a
        // vacuum gives them back to the disk; in a store of a few hundred MB
        // they were most of it.
        conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
            .sql()?;
    }
    if before(found, BEFORE_SHOTS_ON_VOLUME_STAMP) {
        remove_kept_reports(path)?;
    }
    crate::text::note(format!("moved {} to today's schema", path.display()));
    Ok(())
}

/// Deletes the folder `shots` beside the database at <path>, where builds
/// kept the shots runs' reports before it kept the runs on the build volume.
fn remove_kept_reports(path: &Path) -> Result<()> {
    let reports = path
        .parent()
        .expect("a store's database is in its store's folder")
        .join("shots");
    crate::capped_store::remove_all(&reports)
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
pub mod tests;
