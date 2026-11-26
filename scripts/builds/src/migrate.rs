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
use crate::flock::is_held;
use crate::lane::Lane;
use crate::places::Places;

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
            crate::queue::open_with(&old_lock, libc::O_CREAT)
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

/// The stamp of the schema before the shots prefixes dropped from a landing.
const BEFORE_APP_SHOTS_DROP_STAMP: i64 = 1_544_708_784;

/// The stamp of the schema before the agents' acknowledgements stopped being
/// keyed by repo.
const BEFORE_ACK_REPO_STAMP: i64 = 393_821_130;

/// The stamp of the schema before a reading's parts were chained by tokens
/// instead of checked with made-up words.
const BEFORE_ACK_TOKENS_STAMP: i64 = 1_455_084_742;

/// The stamp of the schema before builds stopped keeping the time of an
/// acknowledgement and the sections of a reading, which nothing reads.
const BEFORE_ACK_UNREAD_STAMP: i64 = 1_804_179_352;

/// The stamp of the schema before a review page's entry could be a
/// component's preview.
const BEFORE_REVIEW_COMPONENT_STAMP: i64 = 670_251_313;

/// The stamp of the schema before a design change could delete a component:
/// its deleted places were boards only.
const BEFORE_DELETED_COMPONENT_STAMP: i64 = 290_659_286;

/// The stamp of the schema before builds recorded whether each app build step
/// ran at the merge base.
const BEFORE_APP_BUILD_AT_MERGE_BASE_STAMP: i64 = 47_299_978;

/// The stamp of the schema before the shots steps of an app build kept what
/// they used.
const BEFORE_APP_BUILD_STEP_SHOTS_STAMP: i64 = 1_224_315_788;

/// The stamp before review records could leave token usage and spec-check cost unknown.
const BEFORE_UNREPORTED_USAGE_STAMP: i64 = 1_690_253_367;

/// The stamp before an inbox could address a Codex thread.
const BEFORE_CODEX_INBOX_STAMP: i64 = 807_141_969;

/// The stamp before review calls recorded which findings they kept open.
const BEFORE_REVIEW_KEPT_OPEN_STAMP: i64 = 916_849_912;

/// The stamp before review findings recorded how builds identified the line.
const BEFORE_REVIEW_FINDING_SITE_STAMP: i64 = 124_925_398;

/// The stamp of the schema before a review landing could end abandoned.
const BEFORE_LANDING_ABANDONED_STAMP: i64 = 1_419_870_996;

/// The stamp before app builds recorded the resolved commit they requested.
const BEFORE_APP_BUILD_REQUESTED_COMMIT_STAMP: i64 = 1_541_859_673;

/// The stamp before app builds recorded reused shots comparisons.
const BEFORE_REUSED_SHOTS_STAMP: i64 = 1_189_101_032;

/// The stamp before builds stopped keeping the time at which an agent last
/// acknowledged each version of the text.
const BEFORE_ACK_VERSION_TIME_STAMP: i64 = 1_186_140_394;

/// The stamp before running items recorded the tier of model they take.
const BEFORE_RUNNING_TIER_STAMP: i64 = 1_913_940_341;

/// The stamp before review calls recorded the reason of each verdict that kept
/// a finding open.
const BEFORE_KEPT_OPEN_REASON_STAMP: i64 = 871_098_240;

/// The stamp before the judge could correct the reason it kept a finding.
const BEFORE_KEPT_CORRECTION_STAMP: i64 = 881_234_274;

/// The stamp of the schema before a design change stopped keeping the canvas
/// link of each board (decision #702).
const BEFORE_NO_BOARD_LINK_STAMP: i64 = 285_608_548;

/// The stamp of the schema before `builds agent run` recorded the agents it
/// starts.
const BEFORE_AGENT_RECORD_STAMP: i64 = 204_057_206;

/// The stamp of the schema before the review lost its cap on added lines
/// (decision #746).
const BEFORE_NO_OVER_CAP_STAMP: i64 = 850_242_028;

/// The stamp of the schema before builds recorded the changes of open items.
const BEFORE_ITEM_CHANGE_STAMP: i64 = 1_050_103_093;

/// The stamp of the schema before the backlog (decision #751): the stack had a
/// held kind and records could close as deferred.
const BEFORE_BACKLOG_STAMP: i64 = 1_696_267_334;

/// The stamp of the schema before a finding that a review closed could close
/// again: `review_closed` held one row for each finding.
const BEFORE_REOPEN_STAMP: i64 = 867_290_757;

/// The stamp of today's schema (`db::STAMP`). A change to the schema changes
/// its stamp, and builds then fails to compile until the change gives it a
/// move from this schema, since every store carries this stamp: the change
/// adds the move, puts this stamp at the end of `STAMPS` under a name that
/// says what the schema lacked, and sets this to the new schema's stamp, which
/// the compiler's error names.
const SCHEMA_STAMP: i64 = 1_905_259_196;

// The compiler refuses a schema whose stamp is not SCHEMA_STAMP. A const
// assert could not print the new stamp, so the check compares two array
// lengths, and the compiler's error names both: the array's expected size is
// SCHEMA_STAMP, and the size it found is the new schema's stamp.
const _: [(); SCHEMA_STAMP as usize] = [(); db::STAMP as usize];

/// Every stamp a store can carry that `move_store` moves, oldest schema first.
/// A new move adds the stamp of the schema before it at the end.
const STAMPS: [i64; 52] = [
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
    BEFORE_APP_SHOTS_DROP_STAMP,
    BEFORE_ACK_REPO_STAMP,
    BEFORE_ACK_TOKENS_STAMP,
    BEFORE_ACK_UNREAD_STAMP,
    BEFORE_REVIEW_COMPONENT_STAMP,
    BEFORE_DELETED_COMPONENT_STAMP,
    BEFORE_APP_BUILD_AT_MERGE_BASE_STAMP,
    BEFORE_APP_BUILD_STEP_SHOTS_STAMP,
    BEFORE_UNREPORTED_USAGE_STAMP,
    BEFORE_CODEX_INBOX_STAMP,
    BEFORE_REVIEW_KEPT_OPEN_STAMP,
    BEFORE_REVIEW_FINDING_SITE_STAMP,
    BEFORE_LANDING_ABANDONED_STAMP,
    BEFORE_APP_BUILD_REQUESTED_COMMIT_STAMP,
    BEFORE_REUSED_SHOTS_STAMP,
    BEFORE_ACK_VERSION_TIME_STAMP,
    BEFORE_RUNNING_TIER_STAMP,
    BEFORE_KEPT_OPEN_REASON_STAMP,
    BEFORE_KEPT_CORRECTION_STAMP,
    BEFORE_NO_BOARD_LINK_STAMP,
    BEFORE_AGENT_RECORD_STAMP,
    BEFORE_NO_OVER_CAP_STAMP,
    BEFORE_ITEM_CHANGE_STAMP,
    BEFORE_BACKLOG_STAMP,
    BEFORE_REOPEN_STAMP,
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
        // The indexes as they were then: a later move gives the landings'
        // table its columns and its today's indexes (`add_abandoned`).
        tx.execute_batch(
            "DROP INDEX review_landing_key;
             CREATE INDEX review_landing_change ON review_landing (repo, key);
             CREATE UNIQUE INDEX review_landing_open_once ON review_landing (repo, key) WHERE landed_at IS NULL;",
        )
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
    let project = path
        .parent()
        .and_then(Path::file_name)
        .ok_or_else(|| {
            crate::fail::Fail::Message(format!(
                "builds cannot name the project folder for {}",
                path.display()
            ))
        })?
        .to_string_lossy();
    let texts = crate::review::texts::Texts::of(places, &project);
    if before(found, BEFORE_REVIEW_TEXTS_ON_VOLUME_STAMP) {
        crate::review::record::move_texts_to_volume(&tx, &texts)?;
    }
    // Every store from before the coordinator could mark a branch as
    // mechanical gets the empty table: none was marked yet.
    if before(found, BEFORE_MARKED_MECHANICAL_STAMP) {
        tx.execute_batch(db::MARKED_MECHANICAL_PART).sql()?;
    }
    // Every store from before an agent could drop a shots prefix from a
    // landing gets the empty table: none was dropped yet.
    if before(found, BEFORE_APP_SHOTS_DROP_STAMP) {
        tx.execute_batch(db::APP_SHOTS_DROP_PART).sql()?;
    }
    // Every store from before the acknowledgements stopped being keyed by repo
    // gets the table again with no acknowledgements: the text changed, so each
    // agent reads it again.
    if before(found, BEFORE_ACK_REPO_STAMP) {
        crate::ack::record::drop_repo(&tx)?;
    }
    // Every store from before the readings were chained by tokens loses its
    // readings, which held made-up words, and gets the tokens' table: each
    // agent starts its reading again.
    if before(found, BEFORE_ACK_TOKENS_STAMP) {
        crate::ack::record::replace_words_with_tokens(&tx)?;
    }
    // Every store from before nothing read the time of an acknowledgement or
    // the sections of a reading loses the time and the table of the sections,
    // and keeps each acknowledgement.
    if before(found, BEFORE_ACK_UNREAD_STAMP) {
        crate::ack::record::drop_unread(&tx)?;
    }
    // Every store from before a review page's entry could be a component's
    // preview gets the entry table again with the component's column; each
    // entry keeps its board and theme.
    if before(found, BEFORE_REVIEW_COMPONENT_STAMP) {
        crate::lists::design::add_review_component(&tx)?;
    }
    // Every store from before a design change could delete a component gets
    // the deleted places' table again, which accepts a folder; each row stays.
    if before(found, BEFORE_DELETED_COMPONENT_STAMP) {
        crate::lists::design::allow_deleted_components(&tx)?;
    }
    // A store from before an app build run could end stopped got the steps'
    // table with the column from `record_stopped_runs` above. Every later
    // store from before builds recorded whether each step ran at the merge
    // base gets the steps' table again with the column; each step has NULL,
    // since builds never recorded which it was.
    if !before(found, BEFORE_APP_BUILD_STOPPED_STAMP)
        && before(found, BEFORE_APP_BUILD_AT_MERGE_BASE_STAMP)
    {
        crate::app_timing::add_at_merge_base(&tx)?;
    }
    // Every store from before the shots steps kept what they used gets the
    // empty table: the steps it holds have none.
    if before(found, BEFORE_APP_BUILD_STEP_SHOTS_STAMP) {
        tx.execute_batch(db::APP_BUILD_STEP_SHOTS_PART).sql()?;
    }
    if before(found, BEFORE_UNREPORTED_USAGE_STAMP) {
        crate::lists::spec_check::allow_unreported_usage(&tx)?;
        crate::review::record::allow_unreported_tokens(&tx, &texts)?;
    }
    if before(found, BEFORE_CODEX_INBOX_STAMP) {
        crate::lists::design_session::move_legacy_destination(&tx)?;
        crate::lists::publish::move_legacy_destination(&tx)?;
        if !before(found, BEFORE_JUDGE_STAMP) {
            crate::review::judge::move_legacy_judge(&tx)?;
        }
    }
    if before(found, BEFORE_REVIEW_KEPT_OPEN_STAMP) {
        tx.execute_batch(db::REVIEW_KEPT_OPEN_PART).sql()?;
    }
    if before(found, BEFORE_REVIEW_FINDING_SITE_STAMP) {
        tx.execute_batch(db::REVIEW_FINDING_SITE_PART).sql()?;
    }
    // Every store from before a review landing could end abandoned gets the
    // landings' table again with the columns that say so; each landing keeps
    // its row.
    if before(found, BEFORE_LANDING_ABANDONED_STAMP) {
        crate::review::record::add_abandoned(&tx)?;
    }
    if before(found, BEFORE_APP_BUILD_REQUESTED_COMMIT_STAMP) {
        tx.execute_batch(db::APP_BUILD_REQUESTED_COMMIT_PART)
            .sql()?;
    }
    if before(found, BEFORE_REUSED_SHOTS_STAMP) {
        tx.execute_batch(db::APP_BUILD_REUSED_SHOTS_PART).sql()?;
    }
    // Every store from before builds kept only the versions that agents'
    // acknowledgements name loses the acknowledgements of versions it kept no
    // sections of (their agents read the text again) and the time of each
    // version.
    if before(found, BEFORE_ACK_VERSION_TIME_STAMP) {
        crate::ack::record::drop_version_time(&tx)?;
    }
    // Every store from before running items recorded the tier of model they
    // take gives each running item the implementation tier.
    if before(found, BEFORE_RUNNING_TIER_STAMP) {
        crate::lists::running::add_tier(&tx)?;
    }
    // Every store from before review calls recorded the reason of each
    // verdict that kept a finding open has none for the verdicts it holds.
    if before(found, BEFORE_KEPT_OPEN_REASON_STAMP) {
        crate::review::kept_open::add_reason(&tx)?;
    }
    // Every store from before the judge could correct the reason it kept a
    // finding gets the empty table of corrections.
    if before(found, BEFORE_KEPT_CORRECTION_STAMP) {
        tx.execute_batch(db::REVIEW_KEPT_CORRECTION_PART).sql()?;
    }
    // Every store from before a design change stopped keeping the canvas link
    // of each board loses the links' table: claude.ai is only for publishing,
    // and nothing reads the links.
    if before(found, BEFORE_NO_BOARD_LINK_STAMP) {
        tx.execute_batch("DROP TABLE design_board;").sql()?;
    }
    // Every store from before `builds agent run` recorded the agents it
    // starts gets the empty tables of agents.
    if before(found, BEFORE_AGENT_RECORD_STAMP) {
        tx.execute_batch(db::AGENT_RECORD_PART).sql()?;
    }
    // Every store whose schema held the branches over the review's cap loses
    // their table and closes the questions that still waited on the person
    // about one: the review has no cap now (decision #746).
    if !before(found, BEFORE_OVER_CAP_STAMP) && before(found, BEFORE_NO_OVER_CAP_STAMP) {
        drop_over_cap(&tx, now)?;
    }
    // Every store from before builds recorded the changes of open items gets
    // the empty tables of changes.
    if before(found, BEFORE_ITEM_CHANGE_STAMP) {
        tx.execute_batch(db::ITEM_CHANGE_PART).sql()?;
    }
    // Every store from before the backlog moves the work its held entries
    // gate into the backlog and turns its deferred closings into dropped
    // ones.
    if before(found, BEFORE_BACKLOG_STAMP) {
        move_to_backlog(&tx, now)?;
    }
    // Every store from before a finding that a review closed could close again
    // gets the closes' table again with the key that allows it; each close
    // stays.
    if before(found, BEFORE_REOPEN_STAMP) {
        crate::review::record::allow_reclosing(&tx)?;
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

/// Drops the table of the branches over the review's cap, after it settles
/// each question in it that the person has not answered or closed.
fn drop_over_cap(tx: &Connection, now: i64) -> Result<()> {
    let stamp = crate::lists::Stamp {
        at: now,
        by: "builds install".to_string(),
    };
    let mut statement = tx
        .prepare("SELECT DISTINCT question_id FROM review_over_cap WHERE question_id IS NOT NULL")
        .sql()?;
    let asked: Vec<crate::lists::Id> = statement
        .query_map([], |row| row.get(0))
        .sql()?
        .collect::<std::result::Result<_, _>>()
        .sql()?;
    drop(statement);
    for id in asked {
        if let Some((crate::lists::record::Kind::Question, None)) =
            crate::lists::record::standing(tx, id)?
        {
            crate::lists::record::close(
                tx,
                id,
                crate::lists::record::How::Settled(
                    "the review has no cap on added lines now (decision #746), so no branch waits on this question".to_string(),
                ),
                &stamp,
            )?;
        }
    }
    tx.execute_batch("DROP TABLE review_over_cap;").sql()?;
    Ok(())
}

/// Moves a store from before the backlog to its shape. Each open held entry
/// closes as done, and each open item that waits on it loses that wait and
/// enters the backlog, with the entry's title and details as its why. Every
/// held entry then becomes a your-step entry, and every deferred closing a
/// dropped one that keeps its URL in the reason. Refuses, naming the entry
/// and the item, when an item that waits on an open held entry runs or keeps
/// a worktree, or when an open held entry has no open item waiting on it, and
/// when the moved store would fail its check.
fn move_to_backlog(tx: &Connection, now: i64) -> Result<()> {
    use crate::lists::item_event;
    let stamp = crate::lists::Stamp {
        at: now,
        by: "builds install".to_string(),
    };
    tx.execute_batch(db::statement(
        db::SCHEMA,
        "CREATE TABLE item_backlog (",
        ") STRICT;",
    ))
    .sql()?;
    let entries: Vec<(i64, String, String)> = tx
        .prepare(
            "SELECT id, title, details FROM question WHERE kind = 'held' \
             AND NOT EXISTS (SELECT 1 FROM closing WHERE record_id = question.id) ORDER BY id",
        )
        .sql()?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .sql()?
        .collect::<std::result::Result<_, _>>()
        .sql()?;
    let mut moved = false;
    // The items whose events begin the backlog, recorded once item_event
    // takes that kind.
    let mut entered: Vec<(i64, String)> = Vec::new();
    let mut gated_by: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    for (entry, title, details) in entries {
        let items: Vec<i64> = tx
            .prepare(
                "SELECT w.item_id FROM item_wait_question w WHERE w.on_question = ?1 \
                 AND NOT EXISTS (SELECT 1 FROM closing WHERE record_id = w.item_id) ORDER BY w.item_id",
            )
            .sql()?
            .query_map([entry], |row| row.get(0))
            .sql()?
            .collect::<std::result::Result<_, _>>()
            .sql()?;
        if items.is_empty() {
            fail!(
                "held entry #{entry} \"{title}\" has no open item waiting on it, so the move to the backlog has nothing to move: close it (`builds question drop {entry} --why WHY`), then install again"
            );
        }
        for item in &items {
            let places: i64 = tx
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM running WHERE item_id = ?1) \
                          + (SELECT COUNT(*) FROM kept_work WHERE item_id = ?1)",
                    [item],
                    |row| row.get(0),
                )
                .sql()?;
            if places > 0 {
                fail!(
                    "item #{item} waits on held entry #{entry} \"{title}\" and runs or keeps a worktree, and the backlog keeps no worktree: stop the item (`builds item stop {item} --why WHY`), then install again"
                );
            }
        }
        for item in &items {
            if let Some(first) = gated_by.insert(*item, entry) {
                fail!(
                    "item #{item} waits on two open held entries, #{first} and #{entry}, so the move cannot keep both of their reasons: drop one entry (`builds question drop N --why WHY`), then install again"
                );
            }
        }
        let why = format!("{title}: {details}");
        for item in &items {
            tx.execute(
                "DELETE FROM item_wait_question WHERE item_id = ?1 AND on_question = ?2",
                rusqlite::params![item, entry],
            )
            .sql()?;
            tx.execute(
                "INSERT INTO item_backlog (item_id, why, at, by) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![item, why, stamp.at, stamp.by],
            )
            .sql()?;
            let recorded: bool = tx
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM item_event WHERE item_id = ?1)",
                    [item],
                    |row| row.get(0),
                )
                .sql()?;
            if recorded {
                entered.push((*item, why.clone()));
            }
        }
        let named: Vec<String> = items.iter().map(i64::to_string).collect();
        tx.execute(
            "INSERT INTO closing (record_id, kind, how, at, by, reason) VALUES (?1, 'question', 'done', ?2, ?3, ?4)",
            rusqlite::params![
                entry,
                stamp.at,
                stamp.by,
                format!("moved to the backlog with items {}", named.join(", "))
            ],
        )
        .sql()?;
        moved = true;
    }
    db::remake_table(
        tx,
        "question",
        db::statement(db::SCHEMA, "CREATE TABLE question (", ") STRICT;"),
        |db| {
            db.execute_batch(
                "INSERT INTO question (id, record_kind, title, details, kind, effect, blocks_nothing, origin) \
                 SELECT id, record_kind, title, details, CASE kind WHEN 'held' THEN 'your-step' ELSE kind END, effect, blocks_nothing, origin FROM question_old;",
            )
            .sql()
        },
        &[db::statement(
            db::SCHEMA,
            "CREATE INDEX question_origin ON question",
            ";",
        )],
    )?;
    db::remake_table(
        tx,
        "closing",
        db::statement(db::SCHEMA, "CREATE TABLE closing (", ") STRICT;"),
        |db| {
            db.execute_batch(
                "INSERT INTO closing (record_id, kind, how, at, by, decision_id, question_id, reason) \
                 SELECT record_id, kind, CASE how WHEN 'deferred' THEN 'dropped' ELSE how END, at, by, decision_id, question_id, \
                        CASE how WHEN 'deferred' THEN 'deferred to ' || reason ELSE reason END FROM closing_old;",
            )
            .sql()
        },
        &[],
    )?;
    db::remake_table(
        tx,
        "item_event",
        db::statement(db::SCHEMA, "CREATE TABLE item_event (", ") STRICT;"),
        |db| {
            db.execute_batch(
                "INSERT INTO item_event (id, item_id, at, kind, detail) SELECT id, item_id, at, kind, detail FROM item_event_old;",
            )
            .sql()
        },
        &[db::statement(
            db::SCHEMA,
            "CREATE INDEX item_event_item ON item_event",
            ";",
        )],
    )?;
    for (item, why) in entered {
        item_event::record(
            tx,
            crate::lists::Id(item as u32),
            now,
            item_event::Kind::Backlog,
            Some(&why),
        )?;
    }
    if moved {
        let project: String = tx
            .query_row("SELECT name FROM project", [], |row| row.get(0))
            .sql()?;
        let found = crate::lists::check::faults(tx, &project)?;
        if !found.is_empty() {
            fail!(
                "the move to the backlog leaves the lists failing their check, so it is left undone:\n  {}",
                found.join("\n  ")
            );
        }
    }
    Ok(())
}

/// Whether a legacy exchange stopped before its reviewer reported usage.
pub(crate) fn legacy_failure_had_no_usage(output: &str) -> bool {
    let Some(why) = output.strip_prefix("no answer: ") else {
        return false;
    };
    if why == "the reviewer printed no JSON" {
        return true;
    }
    if why.starts_with("the reviewer ") {
        return false;
    }
    why.starts_with("running ")
        || why.starts_with("waiting for ")
        || why
            .split_once(" exited with ")
            .is_some_and(|(program, status)| {
                !program.is_empty()
                    && (status.starts_with("exit status: ") || status.starts_with("signal: "))
            })
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
