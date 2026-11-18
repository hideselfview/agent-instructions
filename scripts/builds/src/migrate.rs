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

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::db::{self, DATABASE, Sql};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lane::Lane;
use crate::lists::{Effect, Id};
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

/// The stamp of the schema before questions named an origin and an origin
/// could be a record of any kind.
const BEFORE_ORIGINS_STAMP: i64 = 778_443_283;

/// The stamp of the schema before an item could close as finished.
const BEFORE_FINISHED_STAMP: i64 = 1_574_915_211;

/// The stamp of the schema before a review's patch ids left out context lines.
const BEFORE_CHANGED_LINES_STAMP: i64 = 640_728_139;

/// The stamp of the schema before builds kept this Mac's load when each hook
/// check started and recorded each `builds cargo` run.
const BEFORE_CHECK_STARTS_STAMP: i64 = 1_449_009_981;

/// The stamp of the schema before a project said whether it publishes its
/// boards as a canvas.
const BEFORE_CANVAS_STAMP: i64 = 865_754_034;

/// The stamp of the schema before a design change could close as dropped.
const BEFORE_DESIGN_DROP_STAMP: i64 = 693_169_097;

/// The stamp of the schema before builds recorded each commit a landing
/// pushed.
const BEFORE_PUSHED_STAMP: i64 = 627_299_472;

/// The stamp of the schema before each question placed first had a rank in
/// the person's ranked order.
const BEFORE_RANKS_STAMP: i64 = 1_211_427_460;

/// The stamp of the schema before an item could hold a worktree parked, with
/// no agent, and before a dropped item recorded the work it held.
const BEFORE_PARKED_STAMP: i64 = 1_709_769_760;

/// The stamp of the schema before builds recorded each check of a brief
/// against the product spec.
const BEFORE_SPEC_CHECK_STAMP: i64 = 1_463_522_731;

/// The stamp of the schema before the coordinator could settle a finding again
/// and before builds recorded confirmed findings whose code is no longer in
/// their branch's change.
const BEFORE_CODE_LEFT_STAMP: i64 = 1_407_353_448;

/// The stamp of the schema before a later review run read only how the
/// branch's net change differs from the net change an earlier run read.
const BEFORE_NET_CHANGE_STAMP: i64 = 1_962_601_340;

/// The stamp of the schema before each landing named the run of its last
/// review.
const BEFORE_REVIEWED_RUN_STAMP: i64 = 1_758_057_444;

/// The stamp of the schema before a review closed the confirmed findings its
/// changes fix, in place of fix commits that a coordinator named.
const BEFORE_CLOSED_STAMP: i64 = 1_421_793_655;

/// The stamp of the schema before builds counted the lines each review run's
/// diff adds and removes.
const BEFORE_COUNTED_LINES_STAMP: i64 = 847_323_498;

/// The stamp of the schema before a question could give a reason for blocking
/// nothing only while no open item waits on it.
const BEFORE_BLOCKS_FROM_WAITS_STAMP: i64 = 92_039_681;

/// The stamp of the schema before builds recorded every `builds cargo` run
/// with its start and subcommand, and the load of each app build run's host.
const BEFORE_WAITS_STAMP: i64 = 471_804_593;

/// The stamp of the schema before the person could allow a confirmed finding
/// to land.
const BEFORE_ALLOWED_STAMP: i64 = 1_783_029_906;

/// The stamp of the schema before builds kept the agents' acknowledgements of
/// the text it asks them to read.
const BEFORE_ACK_STAMP: i64 = 1_699_272_218;

/// The stamp of the schema before each question named its effect.
const BEFORE_EFFECTS_STAMP: i64 = 1_123_945_986;

/// The file beside a store's builds.sqlite that gives the move to effects
/// each question's effect: a JSON object that maps each question's id to its
/// effect's word, such as `{"515": "data-loss"}`.
pub const QUESTION_EFFECTS: &str = "question-effects.json";

/// The stamp of the schema before builds kept each run of an item that
/// ended, and the spans of each run during which the worktree's claim note
/// said held or the item waited on the person.
const BEFORE_RUN_TIMES_STAMP: i64 = 726_402_770;

/// The stamp of the schema before builds recorded each writing check.
const BEFORE_WRITING_RUNS_STAMP: i64 = 1_268_800_089;

/// The stamp of the schema before builds dropped the writing group's findings
/// on code comments and commit messages.
const BEFORE_MESSAGE_STYLE_STAMP: i64 = 369_927_263;

/// The stamp of the schema before builds named each agent by its Claude Code
/// agent id, kept the versions of the text that agents acknowledged, and
/// recorded the sections a reading holds.
const BEFORE_AGENT_IDS_STAMP: i64 = 280_014_316;

/// The stamp of the schema before builds made the wording check a copy
/// review of its own, which `builds review` never runs.
const BEFORE_COPY_REVIEW_STAMP: i64 = 2_122_037_992;

/// The stamp of the schema before builds kept an agent's acknowledgement of
/// each repo's text apart.
const BEFORE_ACK_PER_REPO_STAMP: i64 = 1_591_868_490;

/// The stamp of the schema before the decided changes' table was added.
const BEFORE_DESIGN_DECISION_STAMP: i64 = 1_679_979_855;

/// The stamp of the schema before a design change's review page was one
/// section.
const BEFORE_ONE_SECTION_STAMP: i64 = 507_266_959;

/// The stamp of the schema before the designer chose the circles on a review
/// board.
const BEFORE_REVIEW_CIRCLES_STAMP: i64 = 1_361_574_994;

/// The stamp of the schema before builds stopped keeping a hash of the groups
/// each review run used.
const BEFORE_RUNS_WITHOUT_GROUPING_STAMP: i64 = 341_419_053;

/// The stamp of the schema before builds named the work an item keeps: the
/// store had `parked` and `parked_place`, and a run could end as parked.
const BEFORE_KEPT_WORK_STAMP: i64 = 169_234_983;

/// The stamp of the schema before builds kept the design session that listens
/// for publish requests, and the requests.
const BEFORE_PUBLISH_STAMP: i64 = 495_639_331;

/// The stamp of the schema before a question could close as done only with a
/// reason.
const BEFORE_DONE_REASON_STAMP: i64 = 845_794_923;

/// Every stamp a store can carry that `move_store` moves, oldest schema first.
/// A new move adds the stamp of the schema before it at the end.
const STAMPS: [i64; 46] = [
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
    BEFORE_ORIGINS_STAMP,
    BEFORE_FINISHED_STAMP,
    BEFORE_CHANGED_LINES_STAMP,
    BEFORE_CHECK_STARTS_STAMP,
    BEFORE_CANVAS_STAMP,
    BEFORE_DESIGN_DROP_STAMP,
    BEFORE_PUSHED_STAMP,
    BEFORE_RANKS_STAMP,
    BEFORE_PARKED_STAMP,
    BEFORE_SPEC_CHECK_STAMP,
    BEFORE_CODE_LEFT_STAMP,
    BEFORE_NET_CHANGE_STAMP,
    BEFORE_REVIEWED_RUN_STAMP,
    BEFORE_CLOSED_STAMP,
    BEFORE_COUNTED_LINES_STAMP,
    BEFORE_BLOCKS_FROM_WAITS_STAMP,
    BEFORE_WAITS_STAMP,
    BEFORE_ALLOWED_STAMP,
    BEFORE_ACK_STAMP,
    BEFORE_EFFECTS_STAMP,
    BEFORE_RUN_TIMES_STAMP,
    BEFORE_WRITING_RUNS_STAMP,
    BEFORE_MESSAGE_STYLE_STAMP,
    BEFORE_AGENT_IDS_STAMP,
    BEFORE_COPY_REVIEW_STAMP,
    BEFORE_ACK_PER_REPO_STAMP,
    BEFORE_DESIGN_DECISION_STAMP,
    BEFORE_ONE_SECTION_STAMP,
    BEFORE_REVIEW_CIRCLES_STAMP,
    BEFORE_RUNS_WITHOUT_GROUPING_STAMP,
    BEFORE_KEPT_WORK_STAMP,
    BEFORE_PUBLISH_STAMP,
    BEFORE_DONE_REASON_STAMP,
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
/// and its rows from every store, and every store gets `item` with an origin of
/// any kind (each item keeps the decision or design change that made it) and
/// `question` with an origin (no question has one yet), and the move gives
/// every commit that a review read, and every fix, the patch id of only its
/// added and removed lines, which it reads from the clone `~/dev/<repo>` of
/// the repo its landing names, and every store older than the canvas setting
/// gets the empty `hook_check_starts` and `builds_cargo_runs` tables (no
/// check of an older run has its starting load, and builds has recorded no
/// cargo run yet) and `project` with `canvas`, set for a project whose design
/// changes hold a canvas link (bae's) and clear for the rest, and every store
/// gets `closing` where an item may close as finished and a design change as
/// dropped, and the move copies every closing into the new table, and every
/// store gets the empty `review_pushed` table (no landing has recorded the
/// commits it pushed yet), and every store gets `question_placement` with a
/// rank for each question placed first, given by when each was asked, so
/// each keeps its place in the stack, and every store older than the parked
/// items gets the empty `parked` and `parked_place` tables (no item is parked
/// yet), and every store older than the parked items gets the empty tables
/// of dropped items' work (no drop has recorded any yet), and every store
/// gets the empty tables of the spec checks (no take has checked a brief
/// yet), and every store gets the empty tables of second settles and of
/// confirmed findings whose code is no longer in their branch's change (no
/// finding has either
/// yet), and every store whose review tables are older than today's gets
/// `review_run` with the earlier run each run read the difference from, which
/// is none for every run (each read a whole net change), and every store whose
/// review tables are older than today's gets `review_landing` with the run of
/// each landing's last review: its newest review run that read the branch's
/// net change (merge-base...head) no earlier run of it read, and every store
/// whose review tables are older than today's gets the empty `review_closed`,
/// where each landing's last review closes each confirmed finding that named
/// a fix commit or whose code builds found no longer in its branch, and loses
/// the tables of fixes, of the commits each run read, of the commits each push
/// landed and of findings whose code left their branch. A store from before a
/// review's patch ids left out context lines keeps its old patch ids until the
/// move drops them. Every store gets `builds_cargo_runs`
/// with each run's start and subcommand: the move gives each run that builds
/// recorded before this move the start of the hook run whose check started
/// it, and no subcommand. Every store gets `app_build_runs` with the load of
/// each run's host, and the move gives each run that builds recorded before
/// this move no load. Every store gets the empty `review_allowed` table (the
/// person has allowed no finding to land yet). Every store gets the empty
/// tables of the agents' acknowledgements (no agent has acknowledged the
/// text yet).
/// In every store older than the move that kept the reason
/// for blocking nothing only while no open item waits on the question, the
/// move clears that reason (`blocks_nothing`) on each open question that an
/// open item waits on. The move makes `question` again in every store from
/// before questions named their effect, with the effect that the store's
/// `QUESTION_EFFECTS` file gives each question, and refuses such a store
/// whose questions and file differ, or whose file is missing while the store
/// holds a question; it reads no file for a newer store. Every store gets the tables
/// of ended runs and spans, with each ended run its write log tells of
/// (`running::move_to_runs`) and <now>, the moment install moves it, as the
/// start of its spans. Every store that had the
/// review's tables gets `review_finding` again, with every row, and the person
/// settles as dropped each finding of the writing group that a branch waits
/// on and that is on the commit messages or on comment lines
/// (`review::record::move_to_message_style`). Every store that has the
/// acknowledgements' tables gets today's, empty: every acknowledgement and
/// reading names an agent by its session and a name, not by its agent id, so
/// every agent reads the whole text once (`ack::record::move_to_agent_ids`).
/// Every store that has the writing check's table loses it, with its runs,
/// and every store that had the review's tables gets `review_run` and
/// `review_finding` again, with every row, and the person's settle as dropped
/// of each finding about wording that a branch waits on
/// (`review::record::move_to_copy_review`). Every store that has the
/// acknowledgements' table of one acknowledgement per agent gets today's,
/// empty, which keeps an agent's acknowledgement of each repo's text apart
/// (`ack::record::move_to_ack_per_repo`). Every store gets the empty
/// `design_decision` table (no design change names a decision yet). Every
/// store's review pages become one section each (`design::move_to_one_review_section`), and
/// the move refuses a store with a design change of several. Every store
/// gets the empty `review_circle` table (no review board has circles yet).
/// Every store that had the review's tables gets `review_run` without the
/// hash of each run's groups, which nothing read
/// (`review::record::move_to_runs_without_grouping`). Every store gets the
/// empty tables of the design session that listens for publish requests and
/// of the requests (no session listens yet).
/// Every store gets `closing` where a question closes as done only with a
/// reason, and each question that closed as done before then gets
/// `DONE_BEFORE_REASONS` as its reason, which names no rule or step
/// (`lists::record::remake_closing`).
/// The moves run in one transaction, the store
/// is stamped as today's schema, and it must then equal a fresh database's schema or the transaction rolls back. A
/// store already at today's stamp, or with no database, is left alone; any
/// other stamp is refused. Delete this once it has run.
pub fn migrate_stores(places: &Places, now: i64) -> Result<()> {
    for store in stores(places)? {
        move_store(places, &store, now)?;
    }
    Ok(())
}

/// The path of each project's `builds.sqlite`, in the order of the projects'
/// names, whether the file exists or not.
fn stores(places: &Places) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(&places.lists) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
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
    Ok(dirs.into_iter().map(|dir| dir.join(DATABASE)).collect())
}

/// Whether every store already has this program's schema, so `migrate_stores`
/// moves none: true when one store at least has a schema and every store that
/// has one has this program's. The function skips a store with no schema yet.
/// When no store has a schema, the function cannot tell which schema the
/// installed program writes, so it returns false.
pub fn every_store_has_this_schema(places: &Places) -> Result<bool> {
    let mut any = false;
    for store in stores(places)? {
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

/// The statements that rebuild `hook_run_checks` as hook_stats.sql writes it,
/// taking the table and its index from today's schema text so the result
/// equals a fresh database's, and keeping a time only for a passed check.
fn check_times() -> String {
    let schema = db::schema();
    let table = db::statement(
        &schema,
        "CREATE TABLE IF NOT EXISTS hook_run_checks",
        "WITHOUT ROWID;",
    );
    let index = db::statement(
        &schema,
        "CREATE INDEX IF NOT EXISTS idx_hook_run_checks_name",
        ";",
    );
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
    db::statement(&db::schema(), "CREATE TABLE design_tip", "STRICT;").to_string()
}

/// The statement that makes `design_fix` as design.sql writes it, taken from
/// today's schema text so the result equals a fresh database's.
fn design_fix() -> String {
    db::statement(&db::schema(), "CREATE TABLE design_fix", "STRICT;").to_string()
}

/// The statement that makes `design_decision` as design.sql writes it, taken
/// from today's schema text so the result equals a fresh database's.
fn design_decision() -> String {
    db::statement(&db::schema(), "CREATE TABLE design_decision", "STRICT;").to_string()
}

/// The statement that makes `review_circle` as design.sql writes it, taken
/// from today's schema text so the result equals a fresh database's.
fn review_circle() -> String {
    db::statement(
        &db::schema(),
        "CREATE TABLE review_circle",
        "STRICT, WITHOUT ROWID;",
    )
    .to_string()
}

/// The effects that the `QUESTION_EFFECTS` file <file> gives, or None when
/// the file does not exist. Fails for a file that is no JSON object of
/// question ids and effects' words.
fn read_effects(file: &Path) -> Result<Option<BTreeMap<Id, Effect>>> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("reading {}: {error}", file.display()),
    };
    let words: BTreeMap<String, String> = serde_json::from_str(&text).map_err(|error| {
        Fail::Message(format!(
            "{} is not a JSON object of question ids and effects: {error}",
            file.display()
        ))
    })?;
    let mut effects = BTreeMap::new();
    for (id, word) in words {
        let Ok(number) = id.parse::<u32>() else {
            fail!("{} names {id:?}, which is no question id", file.display())
        };
        effects.insert(
            Id(number),
            crate::lists::args::parse_named(
                &format!("{}'s question {id}", file.display()),
                &word,
                Effect::NAMES,
            )?,
        );
    }
    Ok(Some(effects))
}

fn move_store(places: &Places, path: &Path, now: i64) -> Result<()> {
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
    // A store from just before the cargo runs' table, or after it, has the
    // review's tables; the move makes them in every older store.
    let had_review_tables = !before(found, BEFORE_REVIEW_STAMP);
    // Off for the moves, so a table made again keeps the references to it as
    // they are written; every reference is checked before the commit.
    conn.pragma_update(None, "foreign_keys", false).sql()?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .sql()?;
    // A store from before reviews ran before landing, or after, has every
    // other part.
    if before(found, BEFORE_DESIGN_FIX_STAMP) {
        if before(found, BEFORE_DESIGN_DELETED_STAMP) {
            tx.execute_batch(DESIGN_DELETED).sql()?;
        }
        if before(found, BEFORE_CHECK_TIMES_STAMP) {
            tx.execute_batch(&check_times()).sql()?;
        }
        if before(found, BEFORE_DESIGN_TIP_STAMP) {
            tx.execute_batch(&design_tip()).sql()?;
        }
        if before(found, BEFORE_APP_TIMING_STAMP) {
            tx.execute_batch(db::APP_TIMING_PART).sql()?;
        }
        // Every part is `IF NOT EXISTS`, so a store that has one keeps it.
        tx.execute_batch(db::DESIGN_SESSION_PART).sql()?;
        tx.execute_batch(db::SHOT_FINDINGS_PART).sql()?;
        // The review's tables are plain `CREATE TABLE`s, so a store that has
        // one fails the move.
        if !had_review_tables {
            tx.execute_batch(db::REVIEW_PART).sql()?;
        }
        tx.execute_batch(&design_fix()).sql()?;
    }
    // A store that has the review's tables has them as they were before
    // reviews ran before landing; one that got them above has today's.
    if had_review_tables && before(found, BEFORE_LANDING_REVIEW_STAMP) {
        crate::review::record::move_to_landing_review(&tx)?;
    }
    // A store that has the review's tables has calls without their prompt
    // and cost; one that got them above has today's.
    if had_review_tables && before(found, BEFORE_SCORED_CALLS_STAMP) {
        crate::review::record::move_to_scored_calls(&tx)?;
    }
    // Of the stores this moves, only those stamped just before builds dropped
    // the cargo runs' table or after it already have the tags' table.
    if before(found, BEFORE_TAGS_STAMP) {
        tx.execute_batch(db::TAG_PART).sql()?;
    }
    // Stores stamped after builds added the cargo runs' table still have it;
    // dropping the table also drops its index.
    tx.execute_batch("DROP TABLE IF EXISTS cargo_runs;").sql()?;
    // Only a store from before an item could close as finished, or after, has
    // today's `item` and `question`.
    if before(found, BEFORE_ORIGINS_STAMP) {
        crate::lists::item::move_to_origins(&tx)?;
        crate::lists::question::move_to_origins(&tx)?;
    }
    // Only a store from before a question could close as done only with a
    // reason has no today's `closing`: it let a design change close as
    // dropped, and a question close as done with no reason, which the move
    // marks as closed before reasons.
    if before(found, BEFORE_DONE_REASON_STAMP) {
        crate::lists::record::remake_closing(&tx)?;
    }
    // A store stamped just before a project said whether it publishes a
    // canvas, or after, already has the `hook_check_starts` and
    // `builds_cargo_runs` tables; no older store has them.
    if before(found, BEFORE_CHECK_STARTS_STAMP) {
        tx.execute_batch(db::HOOK_CHECK_STARTS_PART).sql()?;
        tx.execute_batch(db::CARGO_RUNS_PART).sql()?;
    }
    // A store from just before a design change could close as dropped, or
    // after, has `canvas` on its project; no older store has it.
    if before(found, BEFORE_CANVAS_STAMP) {
        crate::lists::project::move_to_canvas(&tx)?;
    }
    // The review's tables the move made above include `review_pushed`; a
    // store that had them gets it here, except a store from after builds
    // recorded the commits a landing pushed, which has it already.
    if had_review_tables && before(found, BEFORE_PUSHED_STAMP) {
        tx.execute_batch(db::statement(
            crate::review::record::BEFORE_CLOSED,
            "CREATE TABLE review_pushed",
            "WITHOUT ROWID;",
        ))
        .sql()?;
    }
    // Only a store from just before builds could park an item, or after, has
    // a rank on its placements.
    if before(found, BEFORE_RANKS_STAMP) {
        crate::lists::question::move_to_ranks(&tx)?;
    }
    // Only a store from just before builds checked briefs against the spec,
    // or after, has the kept work's and dropped items' tables.
    if before(found, BEFORE_PARKED_STAMP) {
        tx.execute_batch(db::KEPT_WORK_PART).sql()?;
        tx.execute_batch(db::DROPPED_PART).sql()?;
    }
    // Only a store from just before the coordinator could settle a finding
    // again, or after, has the spec checks' tables.
    if before(found, BEFORE_SPEC_CHECK_STAMP) {
        tx.execute_batch(db::SPEC_CHECK_PART).sql()?;
    }
    // The review's tables the move made above include the tables of second
    // settles and of confirmed findings whose code is no longer in their
    // branch's change; a store that had the review's tables gets them here,
    // except a store from after builds recorded them, which has them already.
    if had_review_tables && before(found, BEFORE_CODE_LEFT_STAMP) {
        let schema = db::schema();
        tx.execute_batch(db::statement(
            &schema,
            "CREATE TABLE review_settle_again",
            "STRICT;",
        ))
        .sql()?;
        tx.execute_batch(db::statement(
            crate::review::record::BEFORE_CLOSED,
            "CREATE TABLE review_code_left",
            "STRICT;",
        ))
        .sql()?;
    }
    // A store that had the review's tables from before net changes has runs
    // that each read a whole net change; one that got the tables above has
    // today's `review_run`.
    if had_review_tables && before(found, BEFORE_NET_CHANGE_STAMP) {
        crate::review::record::move_to_net_change(&tx)?;
    }
    // A store that had the review's tables from before landings named their
    // last review gets the last review of each landing; one that got the
    // tables above has no landing.
    if had_review_tables && before(found, BEFORE_REVIEWED_RUN_STAMP) {
        crate::review::record::move_to_reviewed_run(&tx)?;
    }
    // A store that had the review's tables has the tables of fixes and of read
    // and pushed commits: the last review of each landing closes each
    // confirmed finding with a fix, and the move drops those tables.
    if had_review_tables && before(found, BEFORE_CLOSED_STAMP) {
        crate::review::record::move_to_closed(&tx)?;
    }
    // A store from after net changes has `review_run` without the count of
    // each run's changed lines; an older store got today's `review_run` from
    // `move_to_net_change`.
    if had_review_tables
        && !before(found, BEFORE_NET_CHANGE_STAMP)
        && before(found, BEFORE_COUNTED_LINES_STAMP)
    {
        crate::review::record::move_to_counted_lines(&tx)?;
    }
    // In a store from before builds counted lines, every review run that read
    // a branch's whole net change gets the count of its changed lines, read
    // from the clone `~/dev/<repo>` of the repo its landing names, when the
    // clone still holds the run's commits. The move names each run whose
    // commits the clone lacks, since that run keeps no count.
    if had_review_tables && before(found, BEFORE_COUNTED_LINES_STAMP) {
        crate::review::record::count_changed_lines(&tx, |repo, base, head| {
            let clone = places.home.join(crate::host::clone_path(repo));
            if !crate::review::commits::exists(&clone, base)?
                || !crate::review::commits::exists(&clone, head)?
            {
                crate::text::note(format!(
                    "{} lacks the commits of a review run of {repo} ({base}...{head}), so the run keeps no count of its changed lines",
                    clone.display()
                ));
                return Ok(None);
            }
            let diff = crate::review::run::diff_between(&clone, base, head)?.diff;
            Ok(Some(crate::review::check::changed_lines(&diff.text)))
        })?;
    }
    // A store from before builds kept the start and subcommand of each cargo
    // run has `builds_cargo_runs` with hook checks' runs alone, unless the
    // move made today's table above; each run starts when its hook run
    // started.
    if !before(found, BEFORE_CHECK_STARTS_STAMP) && before(found, BEFORE_WAITS_STAMP) {
        crate::cargo_run::move_to_every_run(&tx)?;
    }
    // A store from before the app-build timings got today's `app_build_runs`
    // above; every newer one has runs without their host's load.
    if !before(found, BEFORE_APP_TIMING_STAMP) && before(found, BEFORE_WAITS_STAMP) {
        crate::app_timing::move_to_loads(&tx)?;
    }
    // The review's tables the move made above include `review_allowed`; a
    // store that had the review's tables gets it here.
    if had_review_tables && before(found, BEFORE_ALLOWED_STAMP) {
        tx.execute_batch(db::statement(
            &db::schema(),
            "CREATE TABLE review_allowed",
            "STRICT;",
        ))
        .sql()?;
    }
    // Every store from before the acknowledgements gets their empty tables:
    // no agent has acknowledged the text yet.
    if before(found, BEFORE_ACK_STAMP) {
        tx.execute_batch(db::ACK_PART).sql()?;
    }
    // A store stamped just before questions named their effect already
    // keeps a reason for blocking nothing only while no open item waits on
    // the question; the move clears the reason in every older store.
    if before(found, BEFORE_BLOCKS_FROM_WAITS_STAMP) {
        crate::lists::question::move_to_blocks_from_waits(&tx)?;
    }
    // Every store from before questions named their effect gets `question`
    // again, with the effect the store's `QUESTION_EFFECTS` file gives each
    // question. A newer store's questions hold their effects already, and the
    // move reads no file for it.
    if before(found, BEFORE_EFFECTS_STAMP) {
        let file = path.with_file_name(QUESTION_EFFECTS);
        let given = read_effects(&file)?;
        crate::lists::question::move_to_effects(&tx, given.as_ref().unwrap_or(&BTreeMap::new()))
            .map_err(|fail| {
                let missing = if given.is_none() {
                    ", which does not exist"
                } else {
                    ""
                };
                Fail::Message(format!("{}{missing}: {}", file.display(), fail.text()))
            })?;
    }
    // Every store from before builds kept the runs of items gets their
    // tables, and the runs that its write log tells of.
    if before(found, BEFORE_RUN_TIMES_STAMP) {
        let recovered = crate::lists::running::move_to_runs(&tx, now)?;
        crate::text::note(format!(
            "{}: builds read {} from the write log and left out {}, since the log does not tell when {}; \
             the log names no run's branch, and the store records held and waiting time from now on",
            path.display(),
            crate::text::counted(recovered.runs, "ended run", "ended runs"),
            crate::text::counted(recovered.unended, "run", "runs"),
            if recovered.unended == 1 {
                "it ended"
            } else {
                "they ended"
            }
        ));
    }
    // Every store that had the review's tables gets `review_finding` with
    // today's reasons for a drop, and the person's settle of each writing
    // finding on a commit message or a comment that a branch waits on; one
    // that got the tables above has no finding.
    if had_review_tables && before(found, BEFORE_MESSAGE_STYLE_STAMP) {
        let settled = crate::review::record::move_to_message_style(&tx, now)?;
        crate::text::note(format!(
            "{}: builds settled {} on a commit message or a comment as dropped, in the person's name",
            path.display(),
            crate::text::counted(settled, "writing finding", "writing findings")
        ));
    }
    // A store from before the acknowledgements got today's tables above. Every
    // newer store holds acknowledgements that name no agent id, and gets
    // today's tables, empty.
    if !before(found, BEFORE_ACK_STAMP) && before(found, BEFORE_AGENT_IDS_STAMP) {
        crate::ack::record::move_to_agent_ids(&tx)?;
    }
    // Only a store from just before builds recorded each writing check, or
    // after, has the writing check's table, which builds keeps no more.
    if !before(found, BEFORE_WRITING_RUNS_STAMP) && before(found, BEFORE_COPY_REVIEW_STAMP) {
        tx.execute_batch("DROP TABLE review_writing_run;").sql()?;
    }
    // Every store that had the review's tables gets today's `review_run` and
    // `review_finding`, and the person's settle of each finding about wording
    // that a branch waits on; one that got the tables above has no finding.
    if had_review_tables && before(found, BEFORE_COPY_REVIEW_STAMP) {
        let settled = crate::review::record::move_to_copy_review(&tx, now)?;
        crate::text::note(format!(
            "{}: builds settled {} about wording as dropped, in the person's name",
            path.display(),
            crate::text::counted(settled, "finding", "findings")
        ));
    }
    // A store from before builds named each agent by its id got today's
    // acknowledgements' tables above. Every newer store holds one
    // acknowledgement per agent, and gets today's table, empty.
    if !before(found, BEFORE_AGENT_IDS_STAMP) && before(found, BEFORE_ACK_PER_REPO_STAMP) {
        crate::ack::record::move_to_ack_per_repo(&tx)?;
    }
    // Every store from before the decided changes gets their empty table: no
    // design change names a decision yet.
    if before(found, BEFORE_DESIGN_DECISION_STAMP) {
        tx.execute_batch(&design_decision()).sql()?;
    }
    // Every store from before the one-section review pages gets them.
    if before(found, BEFORE_ONE_SECTION_STAMP) {
        crate::lists::design::move_to_one_review_section(&tx)?;
    }
    // Every store from before the designer chose the circles gets their
    // empty table: no review board has circles yet.
    if before(found, BEFORE_REVIEW_CIRCLES_STAMP) {
        tx.execute_batch(&review_circle()).sql()?;
    }
    // A store from before the copy review got today's `review_run` from
    // `move_to_copy_review`; every newer store that has the review's tables
    // has runs with the hash of their groups.
    if had_review_tables
        && !before(found, BEFORE_COPY_REVIEW_STAMP)
        && before(found, BEFORE_RUNS_WITHOUT_GROUPING_STAMP)
    {
        crate::review::record::move_to_runs_without_grouping(&tx)?;
    }
    // A store that had `parked` (from after items could park) gets the same
    // places in `kept_work`: each parked item keeps the worktrees and
    // branches it was parked in, and the move tells each. An older store got
    // today's tables above.
    if !before(found, BEFORE_PARKED_STAMP) && before(found, BEFORE_KEPT_WORK_STAMP) {
        for (item, places) in crate::lists::kept_work::move_to_kept_work(&tx)? {
            let places = crate::lists::kept_work::places_words(places.iter().map(|(w, b)| (w, b)));
            crate::text::note(format!(
                "{}: item {item} keeps {}, where it was parked",
                path.display(),
                places
            ));
        }
    }
    // A store that had `item_run` before runs could not end as parked gets it
    // again; an older store got today's `item_run` above.
    if !before(found, BEFORE_RUN_TIMES_STAMP) && before(found, BEFORE_KEPT_WORK_STAMP) {
        crate::lists::running::move_to_runs_without_park(&tx)?;
    }
    // Every store from before the publish requests gets their empty tables:
    // no session listens and nobody has asked to publish yet.
    if before(found, BEFORE_PUBLISH_STAMP) {
        tx.execute_batch(db::PUBLISH_PART).sql()?;
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
    crate::text::note(format!("moved {} to today's schema", path.display()));
    Ok(())
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
pub mod tests;
