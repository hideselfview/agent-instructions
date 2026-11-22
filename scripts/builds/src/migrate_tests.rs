use super::*;
use crate::db::DATABASE;
use crate::review::record::{Change, add_landing};
use crate::testing::Scratch;
use rusqlite::Connection;
use std::os::unix::fs::MetadataExt;

/// A volume with an old-layout android lane for repo `demo`, its lock file and
/// an empty queue.
fn old_volume(scratch: &Scratch) -> Places {
    let places = scratch.places();
    let dir = places.root.join("demo");
    std::fs::create_dir_all(dir.join("app-android.queue")).unwrap();
    std::fs::write(dir.join("app-android-checkout"), "side-3\n").unwrap();
    std::fs::write(dir.join("app-android.lock"), "").unwrap();
    places
}

/// Waits until nothing holds the lock on <path>. A process another test forks
/// meanwhile has a copy of every open descriptor until it execs, and with it
/// the lock, for a moment after the test closed its own.
fn until_free(path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while is_held(path).unwrap() {
        assert!(
            std::time::Instant::now() < deadline,
            "{} stayed held",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn the_old_files_become_the_new_ones_with_the_lock_keeping_its_inode() {
    let scratch = Scratch::new("migrate");
    let places = old_volume(&scratch);
    let dir = places.root.join("demo");
    let inode = std::fs::metadata(dir.join("app-android.lock"))
        .unwrap()
        .ino();
    migrate_app_files(&places).unwrap();
    assert_eq!(
        places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["side-3"]
    );
    assert!(!dir.join("app-android-checkout").exists() && !dir.join("app-android.lock").exists());
    let lock = places.app_lock_file("demo", Lane::Android, "side-3");
    assert_eq!(
        std::fs::metadata(lock).unwrap().ino(),
        inode,
        "the lock a holder has open is the new lock"
    );
    // Again: nothing left to move.
    migrate_app_files(&places).unwrap();
}

#[test]
fn builds_from_before_keep_their_lock_and_queue_and_the_old_names_go_when_they_end() {
    let scratch = Scratch::new("migrate-busy");
    let places = old_volume(&scratch);
    let dir = places.root.join("demo");
    // An old build holds the old lock.
    let held = crate::queue::open_with(&dir.join("app-android.lock"), libc::O_CLOEXEC).unwrap();
    migrate_app_files(&places).unwrap();
    let new_lock = places.app_lock_file("demo", Lane::Android, "side-3");
    assert_eq!(
        places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["side-3"]
    );
    assert!(
        dir.join("app-android-checkout").is_file() && dir.join("app-android.lock").is_file(),
        "the old names stay"
    );
    assert!(
        crate::queue::try_lock(&new_lock).unwrap().is_none(),
        "the new path is the lock the old build holds"
    );
    unsafe { libc::close(held) };
    until_free(&dir.join("app-android.lock"));

    // A ticket of an old waiter keeps them too.
    let ticket = crate::queue::open_with(
        &dir.join("app-android.queue/123-1"),
        libc::O_CREAT | libc::O_CLOEXEC,
    )
    .unwrap();
    migrate_app_files(&places).unwrap();
    assert!(dir.join("app-android.lock").is_file());
    unsafe { libc::close(ticket) };
    until_free(&dir.join("app-android.queue/123-1"));

    migrate_app_files(&places).unwrap();
    assert!(!dir.join("app-android-checkout").exists() && !dir.join("app-android.lock").exists());
    assert_eq!(
        places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["side-3"]
    );
    let fd = crate::queue::try_lock(&new_lock)
        .unwrap()
        .expect("free once the old build ended");
    unsafe { libc::close(fd) };
}

#[test]
fn a_lane_never_locked_gets_its_old_lock_made_before_it_is_linked() {
    let scratch = Scratch::new("migrate-fresh");
    let places = scratch.places();
    let dir = places.root.join("demo");
    std::fs::create_dir_all(dir.join("app.queue")).unwrap();
    std::fs::write(dir.join("app-checkout"), "side-1\n").unwrap();
    // A waiter of an older `builds` holds a ticket, so the old name stays and
    // must be the file the new path is.
    let ticket =
        crate::queue::open_with(&dir.join("app.queue/9-1"), libc::O_CREAT | libc::O_CLOEXEC)
            .unwrap();
    migrate_app_files(&places).unwrap();
    assert_eq!(
        std::fs::metadata(dir.join("app.lock")).unwrap().ino(),
        std::fs::metadata(places.app_lock_file("demo", Lane::Mac, "side-1"))
            .unwrap()
            .ino()
    );
    unsafe { libc::close(ticket) };
}

/// The index that found a review's landing by its key alone, as record.sql
/// wrote it before builds found a landing by its repo and key.
const LANDING_KEY_INDEX: &str = "CREATE INDEX review_landing_key ON review_landing (key);\n";

/// The indexes that find a review's landing by its repo and key, as
/// record.sql writes them.
const LANDING_CHANGE_INDEXES: &str = "CREATE INDEX review_landing_change ON review_landing (repo, key);\n-- A change has one landing under review at a time.\nCREATE UNIQUE INDEX review_landing_open_once ON review_landing (repo, key) WHERE landed_at IS NULL;\n";

/// The review section's heading, the last column of its table as design.sql
/// writes it.
const REVIEW_SECTION_HEADING: &str = "    heading TEXT NOT NULL CONSTRAINT review_section_heading_says_something CHECK (trim(heading) <> '')\n) STRICT;\n";

/// <schema>, which holds today's design tables, as every schema from before
/// the review section kept only its heading: the section kept the review
/// page's words too.
pub fn with_review_section_words(schema: String) -> String {
    assert_eq!(schema.matches(REVIEW_SECTION_HEADING).count(), 1);
    schema.replacen(
        REVIEW_SECTION_HEADING,
        "    heading TEXT NOT NULL CONSTRAINT review_section_heading_says_something CHECK (trim(heading) <> ''),\n    words TEXT NOT NULL\n) STRICT;\n",
        1,
    )
}

/// The tables of the variants of a design change, as design_variants.sql
/// wrote them before they left the schema.
const DESIGN_VARIANTS_TABLES: &str = "CREATE TABLE design_variants (
    design_id INTEGER PRIMARY KEY REFERENCES design (id) ON DELETE CASCADE,
    board TEXT NOT NULL CONSTRAINT design_variants_board_is_a_board_file CHECK (board GLOB '*.dc.html' AND board NOT GLOB '*#*' AND board NOT GLOB '*/'),
    selector TEXT NOT NULL CONSTRAINT design_variants_selector_says_something CHECK (trim(selector) <> ''),
    theme TEXT NOT NULL CONSTRAINT design_variants_theme_known CHECK (theme IN ('light', 'dark')),
    tip TEXT NOT NULL CONSTRAINT design_variants_tip_is_a_commit CHECK (length(tip) = 40 AND tip NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE design_variant (
    design_id INTEGER NOT NULL,
    position INTEGER NOT NULL CONSTRAINT design_variant_position_not_negative CHECK (position >= 0),
    props TEXT NOT NULL CONSTRAINT design_variant_props_are_an_object CHECK (CASE WHEN json_valid(props) THEN json_type(props) = 'object' ELSE 0 END),
    PRIMARY KEY (design_id, position),
    CONSTRAINT design_variant_is_of_the_variants FOREIGN KEY (design_id) REFERENCES design_variants (design_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;
";

/// <schema>, which holds today's parts up to the circles that surround a
/// box, as every schema from after the variants of a design change joined
/// it until they left: their tables stood after those circles, before the
/// branches over the review's cap if <schema> holds them.
fn with_design_variants(schema: String) -> String {
    match schema.find(db::OVER_CAP_PART) {
        Some(at) => format!(
            "{}{DESIGN_VARIANTS_TABLES}\n{}",
            &schema[..at],
            &schema[at..]
        ),
        None => format!("{schema}\n{DESIGN_VARIANTS_TABLES}"),
    }
}

/// <schema>, which holds today's review tables, as a schema from before
/// builds found a review's landing by its repo and key: with the index on
/// the key alone, and the review's texts in a table without row ids, since
/// that move came later.
fn before_landing_repo(schema: String) -> String {
    let schema = before_review_text_rowid(schema);
    assert_eq!(schema.matches(LANDING_CHANGE_INDEXES).count(), 1);
    schema.replacen(LANDING_CHANGE_INDEXES, LANDING_KEY_INDEX, 1)
}

/// The end of the review's texts' table, as record.sql writes it.
const REVIEW_TEXT_END: &str = "    text TEXT NOT NULL\n) STRICT;\n";

/// <schema>, which holds today's review tables, as a schema from before the
/// review's texts sat in a table with row ids.
pub fn before_review_text_rowid(schema: String) -> String {
    assert_eq!(schema.matches(REVIEW_TEXT_END).count(), 1);
    schema.replacen(
        REVIEW_TEXT_END,
        "    text TEXT NOT NULL\n) STRICT, WITHOUT ROWID;\n",
        1,
    )
}

/// The check that lets a design place name no app work yet, as design.sql
/// writes it.
const PLACE_NAMING_CHECK: &str =
    "CONSTRAINT design_place_is_resolved_one_way CHECK (item_id IS NULL OR matches IS NULL)";

/// <schema>, which holds today's design tables, as a schema from before a
/// design place could name no app work yet: the place resolves one way.
pub fn before_design_place_naming(schema: String) -> String {
    assert_eq!(schema.matches(PLACE_NAMING_CHECK).count(), 1);
    schema.replacen(
        PLACE_NAMING_CHECK,
        "CONSTRAINT design_place_is_resolved_one_way CHECK ((item_id IS NULL) <> (matches IS NULL))",
        1,
    )
}

/// The column of a review board that says whether the page shows its now,
/// as design.sql writes it.
const REVIEW_BOARD_SHOW_NOW: &str = "    show_now INTEGER NOT NULL CONSTRAINT review_board_show_now_is_a_flag CHECK (show_now IN (0, 1)),\n";

/// <schema>, which holds today's review board table, as a schema from before
/// a review board said whether the page shows its now.
pub fn before_review_board_show_now(schema: String) -> String {
    assert_eq!(schema.matches(REVIEW_BOARD_SHOW_NOW).count(), 1);
    schema.replacen(REVIEW_BOARD_SHOW_NOW, "", 1)
}

/// The stamp of <schema>, a schema older than today's that holds today's
/// review board table and app build tables, less the column that says
/// whether the page shows a board's now and how an app build run ended,
/// which every older schema lacked.
fn stamp_before_show_now(schema: String) -> i64 {
    db::stamp_of_text(&before_review_board_show_now(before_app_build_stopped(
        schema,
    )))
}

/// How an app build run ended, as app_timing.sql writes the column, with the
/// end of the comment above it, since a cargo run's outcome has the same
/// column.
const APP_BUILD_OUTCOME: &str = "    -- SIGHUP) reached it before it passed.\n    outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed', 'stopped')),\n";

/// The results an app build step takes, as app_timing.sql writes them.
const APP_BUILD_STEP_RESULTS: &str =
    "result IN ('passed', 'failed', 'timed out', 'taken over', 'stopped')";

/// <schema>, which holds today's app build tables, as a schema from before an
/// app build run could end stopped: a run says whether it passed, and a step
/// cannot be stopped.
pub fn before_app_build_stopped(schema: String) -> String {
    assert_eq!(schema.matches(APP_BUILD_OUTCOME).count(), 1);
    assert_eq!(schema.matches(APP_BUILD_STEP_RESULTS).count(), 1);
    schema
        .replacen(
            APP_BUILD_OUTCOME,
            "    passed INTEGER NOT NULL CHECK (passed IN (0, 1)),\n",
            1,
        )
        .replacen(
            APP_BUILD_STEP_RESULTS,
            "result IN ('passed', 'failed', 'timed out', 'taken over')",
            1,
        )
}

/// The kinds a landing's step takes, as land_run.sql writes them.
const LAND_STEP_KINDS: &str = "kind IN ('lock', 'lists-check', 'app-wait', 'fetch', 'pick', 'checks', 'app-build', 'push', 'records', 'finish')";

/// <schema>, which holds today's landing tables, as a schema from before a
/// landing's steps took the app wait and the app build.
pub fn before_land_app_steps(schema: String) -> String {
    assert_eq!(schema.matches(LAND_STEP_KINDS).count(), 1);
    schema.replacen(
        LAND_STEP_KINDS,
        "kind IN ('lock', 'lists-check', 'fetch', 'pick', 'checks', 'push', 'records', 'finish')",
        1,
    )
}

/// The column that names who ends a condition, as item.sql writes it.
const CONDITION_ENDER_COLUMN: &str = "    until TEXT NOT NULL CONSTRAINT item_wait_condition_ender_known CHECK (until IN ('agent', 'coordinator', 'ci')),\n";

/// The schema text before a condition named who ends it: the schema before
/// the item events without the ender's column.
fn schema_before_condition_enders() -> String {
    let schema = before_landing_repo(with_review_section_words(db::schema_before_item_event()));
    assert_eq!(schema.matches(CONDITION_ENDER_COLUMN).count(), 1);
    schema.replacen(CONDITION_ENDER_COLUMN, "", 1)
}

/// Creates a store in <dir> with the schema from before the judge: that
/// schema's text and its stamp. Install moves it to today's schema.
pub fn store_before_judge(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&before_app_build_stopped(before_landing_repo(
        with_review_section_words(db::schema_before_judge()),
    )))
    .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_JUDGE_STAMP)
        .unwrap();
    path
}

fn has_judge(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'review_judge'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

/// A store whose stamp no move knows, such as the stamp of a schema older
/// than the first move, is refused with its stamp and left as it was.
#[test]
fn a_store_with_a_stamp_no_move_knows_is_refused_and_left_as_it_was() {
    let scratch = Scratch::new("move-unknown");
    let places = scratch.places();
    let path = store_before_judge(&places.lists.join("bae"));
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 882_817_755)
        .unwrap();
    let refused = migrate_stores(&places, 0).unwrap_err().text();
    assert!(
        refused.contains("stamp 882817755") && refused.contains("does not know"),
        "{refused}"
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        (has_judge(&conn), db::stamp_of(&conn, &path).unwrap()),
        (false, 882_817_755)
    );
}

#[test]
fn a_move_that_would_not_equal_a_fresh_schema_rolls_back() {
    let scratch = Scratch::new("move-drift");
    let places = scratch.places();
    let path = store_before_judge(&places.lists.join("bae"));
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP INDEX idx_hook_runs_started")
        .unwrap();
    let refused = migrate_stores(&places, 0).unwrap_err().text();
    assert!(
        refused.contains("not the one this builds makes"),
        "{refused}"
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        (has_judge(&conn), db::stamp_of(&conn, &path).unwrap()),
        (false, BEFORE_JUDGE_STAMP)
    );
}

#[test]
fn each_stamp_names_one_schema_and_before_counts_the_stamp_and_older_ones() {
    let distinct: std::collections::BTreeSet<i64> = STAMPS.iter().copied().collect();
    assert_eq!(distinct.len(), STAMPS.len());
    assert!(before(BEFORE_LAND_RUN_STAMP, BEFORE_LAND_RUN_STAMP));
    assert!(before(BEFORE_ITEM_EVENT_STAMP, BEFORE_LAND_RUN_STAMP));
    assert!(!before(BEFORE_HOST_SAMPLE_STAMP, BEFORE_LAND_RUN_STAMP));
}

#[test]
fn the_item_event_stamp_is_the_schema_before_the_item_events() {
    assert_eq!(
        stamp_before_show_now(before_design_place_naming(before_landing_repo(
            with_review_section_words(db::schema_before_item_event())
        ))),
        BEFORE_ITEM_EVENT_STAMP
    );
}

/// A store from before the item events gains their empty table and the mark of
/// the move's time, and equals a fresh schema.
#[test]
fn a_store_from_before_the_item_events_gains_their_table_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-item-event");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_landing_repo(
            with_review_section_words(db::schema_before_item_event()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_ITEM_EVENT_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 1_790_000_000).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let events: i64 = conn
        .query_row("SELECT count(*) FROM item_event", [], |row| row.get(0))
        .unwrap();
    assert_eq!(events, 0);
    assert_eq!(
        crate::lists::item_event::read_from(&conn).unwrap(),
        Some(1_790_000_000)
    );
}

#[test]
fn the_land_run_stamp_is_the_schema_before_the_landings_record() {
    assert_eq!(
        stamp_before_show_now(before_design_place_naming(before_landing_repo(
            with_review_section_words(db::schema_before_land_run())
        ))),
        BEFORE_LAND_RUN_STAMP
    );
}

/// A store from before the landings' record gains their empty tables and
/// equals a fresh schema.
#[test]
fn a_store_from_before_the_landings_record_gains_their_tables_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-land-run");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_landing_repo(
            with_review_section_words(db::schema_before_land_run()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_LAND_RUN_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let runs: i64 = conn
        .query_row("SELECT count(*) FROM land_run", [], |row| row.get(0))
        .unwrap();
    assert_eq!(runs, 0);
}

#[test]
fn the_exchange_time_stamp_is_the_schema_before_the_reviewer_processes_times() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_landing_repo(with_review_section_words(db::schema_before_exchange_time()))
        ))),
        BEFORE_EXCHANGE_TIME_STAMP
    );
}

/// A store from before the reviewer processes' times gains their empty table
/// and equals a fresh schema.
#[test]
fn a_store_from_before_the_exchange_times_gains_their_table_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-exchange-time");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_landing_repo(
            with_review_section_words(db::schema_before_exchange_time()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_EXCHANGE_TIME_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let times: i64 = conn
        .query_row("SELECT count(*) FROM review_exchange_time", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(times, 0);
}

#[test]
fn the_host_sample_stamp_is_the_schema_before_the_hosts_samples() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_landing_repo(with_review_section_words(db::schema_before_host_sample()))
        ))),
        BEFORE_HOST_SAMPLE_STAMP
    );
}

/// A store from before the hosts' samples gains their empty table and equals
/// a fresh schema.
#[test]
fn a_store_from_before_the_host_samples_gains_their_table_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-host-sample");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_landing_repo(
            with_review_section_words(db::schema_before_host_sample()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_HOST_SAMPLE_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let samples: i64 = conn
        .query_row("SELECT count(*) FROM host_sample", [], |row| row.get(0))
        .unwrap();
    assert_eq!(samples, 0);
}

#[test]
fn the_cargo_run_crate_stamp_is_the_schema_before_the_cargo_runs_crates() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_landing_repo(with_review_section_words(
                db::schema_before_cargo_run_crate()
            ))
        ))),
        BEFORE_CARGO_RUN_CRATE_STAMP
    );
}

/// A store from before the cargo runs' crates gains their empty table and
/// equals a fresh schema.
#[test]
fn a_store_from_before_the_cargo_run_crates_gains_their_table_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-cargo-run-crate");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_landing_repo(
            with_review_section_words(db::schema_before_cargo_run_crate()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_CARGO_RUN_CRATE_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let crates: i64 = conn
        .query_row("SELECT count(*) FROM builds_cargo_run_crate", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(crates, 0);
}

#[test]
fn the_condition_enders_stamp_is_the_schema_before_conditions_named_an_ender() {
    assert_eq!(
        stamp_before_show_now(before_design_place_naming(schema_before_condition_enders())),
        BEFORE_CONDITION_ENDERS_STAMP
    );
}

/// A store from before conditions named an ender gives each condition the
/// coordinator as its ender and equals a fresh schema.
#[test]
fn a_store_from_before_condition_enders_gives_each_condition_the_coordinator() {
    let scratch = Scratch::new("move-condition-enders");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(schema_before_condition_enders()))
            .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_CONDITION_ENDERS_STAMP)
            .unwrap();
        use crate::lists::fixtures::v;
        crate::lists::area::plant(&conn, &[("name", v("lists")), ("position", v(0))]).unwrap();
        crate::lists::record::plant(
            &conn,
            "record",
            &[
                ("id", v(1)),
                ("kind", v("item")),
                ("opened_at", v(5)),
                ("opened_by", v("tester")),
            ],
        )
        .unwrap();
        crate::lists::item::plant(
            &conn,
            "item",
            &[
                ("id", v(1)),
                ("record_kind", v("item")),
                ("title", v("Wait for a landing")),
                ("area", v("lists")),
                ("details", v("d")),
                ("audience", v("coordinator")),
            ],
        )
        .unwrap();
        crate::lists::item::plant(
            &conn,
            "item_wait_condition",
            &[
                ("item_id", v(1)),
                ("condition", v("builds lands the change")),
            ],
        )
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let enders: Vec<(String, String)> = conn
        .prepare("SELECT condition, until FROM item_wait_condition")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        enders,
        [(
            "builds lands the change".to_string(),
            "coordinator".to_string()
        )]
    );
}

#[test]
fn the_judge_stamp_is_the_schema_before_the_judge() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_landing_repo(with_review_section_words(db::schema_before_judge()))
        ))),
        BEFORE_JUDGE_STAMP
    );
}

/// A store from before the judge gains its empty table and equals a fresh
/// schema.
#[test]
fn a_store_from_before_the_judge_gains_its_table_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-judge");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_landing_repo(
            with_review_section_words(db::schema_before_judge()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_JUDGE_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let judges: i64 = conn
        .query_row("SELECT count(*) FROM review_judge", [], |row| row.get(0))
        .unwrap();
    assert_eq!(judges, 0);
}

#[test]
fn the_landing_repo_stamp_is_the_schema_before_landings_were_found_by_repo() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_landing_repo(with_review_section_words(db::schema_before_card_state()))
        ))),
        BEFORE_LANDING_REPO_STAMP
    );
}

/// A store from before builds found a review's landing by its repo and key,
/// with that schema's text and stamp: project `bae` over repos `bae` and
/// `coven`, and an open landing of `branch:work` for each repo that
/// <landings> names, opened at 1, 2 and on. The owners' own writes make the
/// rows: that schema's project and landing tables are the ones `db::schema`
/// makes, and only the landings' indexes differ.
fn store_before_landing_repo(places: &Places, landings: &[&str]) -> std::path::PathBuf {
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&before_app_build_stopped(before_landing_repo(
        with_review_section_words(db::schema_before_card_state()),
    )))
    .unwrap();
    crate::lists::project::init(
        &conn,
        &crate::lists::project::NewProject {
            name: "bae".into(),
            canvas: true,
            repos: vec!["bae".into(), "coven".into()],
            places: Default::default(),
            sources: Default::default(),
            specs: Default::default(),
        },
    )
    .unwrap();
    for (at, repo) in (1..).zip(landings) {
        let change = Change {
            repo,
            key: "branch:work",
        };
        add_landing(&conn, change, at).unwrap();
    }
    conn.pragma_update(None, "user_version", BEFORE_LANDING_REPO_STAMP)
        .unwrap();
    path
}

/// A store from before builds found a review's landing by its repo and key
/// gains the indexes that find it so, keeps its landings, and equals a fresh
/// schema; the database then refuses a second open landing of one change.
#[test]
fn a_store_from_before_landings_were_found_by_repo_gains_their_indexes() {
    let scratch = Scratch::new("move-landing-repo");
    let places = scratch.places();
    let path = store_before_landing_repo(&places, &["bae", "coven"]);
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let landings: i64 = conn
        .query_row("SELECT count(*) FROM review_landing", [], |row| row.get(0))
        .unwrap();
    assert_eq!(landings, 2);
    let change = Change {
        repo: "bae",
        key: "branch:work",
    };
    let second = add_landing(&conn, change, 3).unwrap_err().text();
    assert!(second.contains("UNIQUE constraint failed"), "{second}");
}

/// A store with two open landings of one change fails the move, which
/// leaves it as it was.
#[test]
fn a_store_with_two_open_landings_of_one_change_is_left_as_it_was() {
    let scratch = Scratch::new("move-landing-repo-twice");
    let places = scratch.places();
    let path = store_before_landing_repo(&places, &["bae", "bae"]);
    let refused = migrate_stores(&places, 0).unwrap_err().text();
    assert!(refused.contains("UNIQUE constraint failed"), "{refused}");
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        db::stamp_of(&conn, &path).unwrap(),
        BEFORE_LANDING_REPO_STAMP
    );
}

#[test]
fn the_card_states_stamp_is_the_schema_before_them() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_review_text_rowid(with_review_section_words(db::schema_before_card_state()))
        ))),
        BEFORE_CARD_STATE_STAMP
    );
}

/// A store from before the cards' states gains their empty tables and equals
/// a fresh schema.
#[test]
fn a_store_from_before_the_card_states_gains_their_tables_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-card-states");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_review_text_rowid(
            with_review_section_words(db::schema_before_card_state()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_CARD_STATE_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_cargo_run_outcomes_stamp_is_the_schema_before_them() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_review_text_rowid(with_review_section_words(
                db::schema_before_cargo_run_outcome()
            ))
        ))),
        BEFORE_CARGO_RUN_OUTCOME_STAMP
    );
}

/// A store from before the cargo runs' outcomes gains their empty tables and
/// equals a fresh schema.
#[test]
fn a_store_from_before_the_cargo_run_outcomes_gains_their_tables_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-cargo-run-outcomes");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_review_text_rowid(
            with_review_section_words(db::schema_before_cargo_run_outcome()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_CARGO_RUN_OUTCOME_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_step_commits_stamp_is_the_schema_before_them() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_review_text_rowid(with_review_section_words(db::schema_before_step_commits()))
        ))),
        BEFORE_STEP_COMMITS_STAMP
    );
}

/// A store from before the steps' commits gains their empty tables and equals
/// a fresh schema.
#[test]
fn a_store_from_before_the_step_commits_gains_their_tables_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-step-commits");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_review_text_rowid(
            with_review_section_words(db::schema_before_step_commits()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_STEP_COMMITS_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_review_text_stamp_is_the_schema_before_the_texts_had_row_ids() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            before_review_text_rowid(with_review_section_words(
                db::schema_before_hook_check_failure()
            ))
        ))),
        BEFORE_REVIEW_TEXT_ROWID_STAMP
    );
}

#[test]
fn the_hook_check_failures_stamp_is_the_schema_before_them() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            with_review_section_words(db::schema_before_hook_check_failure())
        ))),
        BEFORE_HOOK_CHECK_FAILURE_STAMP
    );
}

/// A store from before the failed hook checks' words gains their empty tables
/// and equals a fresh schema.
#[test]
fn a_store_from_before_the_hook_check_failures_gains_their_tables_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-hook-check-failures");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(with_review_section_words(
            db::schema_before_hook_check_failure(),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_HOOK_CHECK_FAILURE_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_app_build_step_failures_stamp_is_the_schema_before_them() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            with_review_section_words(db::schema_before_app_build_step_failure())
        ))),
        BEFORE_APP_BUILD_STEP_FAILURE_STAMP
    );
}

/// A store from before the failed app build steps' words gains their empty
/// tables and equals a fresh schema.
#[test]
fn a_store_from_before_the_app_build_step_failures_gains_their_tables_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-app-build-step-failures");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(with_review_section_words(
            db::schema_before_app_build_step_failure(),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_APP_BUILD_STEP_FAILURE_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_judge_told_stamp_is_the_schema_before_it() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            with_review_section_words(db::schema_before_judge_told())
        ))),
        BEFORE_JUDGE_TOLD_STAMP
    );
}

/// A store from before builds recorded which findings it told the judge of
/// gains the empty table and equals a fresh schema.
#[test]
fn a_store_from_before_the_judge_told_table_gains_it_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-judge-told");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(with_review_section_words(
            db::schema_before_judge_told(),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_JUDGE_TOLD_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_design_circle_box_stamp_is_the_schema_before_it() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            with_review_section_words(db::schema_before_design_circle_box())
        ))),
        BEFORE_DESIGN_CIRCLE_BOX_STAMP
    );
}

/// A store from before the circles around a box gains the empty table and
/// equals a fresh schema.
#[test]
fn a_store_from_before_the_design_circle_box_table_gains_it_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-design-circle-box");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(with_review_section_words(
            db::schema_before_design_circle_box(),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_DESIGN_CIRCLE_BOX_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_design_place_naming_stamp_is_the_schema_before_it() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(before_design_place_naming(
            with_review_section_words(db::schema_before_design_variants())
        ))),
        BEFORE_DESIGN_PLACE_NAMING_STAMP
    );
}

/// A store from before a design place could name no app work yet keeps its
/// places and equals a fresh schema.
#[test]
fn a_store_from_before_design_places_named_no_app_work_keeps_its_places_and_equals_a_fresh_schema()
{
    let scratch = Scratch::new("move-design-place-naming");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_design_place_naming(
            with_review_section_words(db::schema_before_design_variants()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_DESIGN_PLACE_NAMING_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_design_variants_stamp_is_the_schema_before_it() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(with_review_section_words(
            db::schema_before_design_variants()
        ))),
        BEFORE_DESIGN_VARIANTS_STAMP
    );
}

/// A store from before the variants of a design change equals a fresh
/// schema, which holds none of their tables either.
#[test]
fn a_store_from_before_the_design_variants_tables_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-design-variants");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(with_review_section_words(
            db::schema_before_design_variants(),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_DESIGN_VARIANTS_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_over_cap_stamp_is_the_schema_before_it() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(with_review_section_words(
            with_design_variants(db::schema_before_over_cap())
        ))),
        BEFORE_OVER_CAP_STAMP
    );
}

/// A store from before the branches over the review's cap gains the empty
/// table and equals a fresh schema.
#[test]
fn a_store_from_before_the_over_cap_table_gains_it_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-over-cap");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(with_review_section_words(
            with_design_variants(db::schema_before_over_cap()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_OVER_CAP_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_review_section_heading_only_stamp_is_the_schema_before_it() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(with_review_section_words(
            with_design_variants(db::with_review_texts(db::with_shot_findings(
                db::schema_before_marked_mechanical()
            )))
        ))),
        BEFORE_REVIEW_SECTION_HEADING_ONLY_STAMP
    );
}

/// A store from before the review section kept only its heading drops the
/// words, keeps each change's heading and equals a fresh schema.
#[test]
fn a_store_from_before_the_review_section_kept_only_its_heading_keeps_each_heading() {
    let scratch = Scratch::new("move-review-section-heading");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(with_review_section_words(
            with_design_variants(db::with_review_texts(db::with_shot_findings(
                db::schema_before_marked_mechanical(),
            ))),
        )))
        .unwrap();
        conn.pragma_update(
            None,
            "user_version",
            BEFORE_REVIEW_SECTION_HEADING_ONLY_STAMP,
        )
        .unwrap();
        use crate::lists::fixtures::v;
        crate::lists::project::plant(&conn, "repo", &[("name", v("bae"))]).unwrap();
        crate::lists::record::plant(
            &conn,
            "record",
            &[
                ("id", v(1)),
                ("kind", v("design")),
                ("opened_at", v(5)),
                ("opened_by", v("tester")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design",
            &[
                ("id", v(1)),
                ("title", v("People rows")),
                ("details", v("d")),
                ("repo", v("bae")),
                ("worktree", v("/w")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "review_section",
            &[
                ("design_id", v(1)),
                ("heading", v("Rows")),
                ("words", v("The rows")),
            ],
        )
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let headings: Vec<(i64, String)> = conn
        .prepare("SELECT design_id, heading FROM review_section")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(headings, [(1, "Rows".to_string())]);
}

#[test]
fn the_design_variants_left_stamp_is_the_schema_before_it() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(with_design_variants(
            db::with_review_texts(db::with_shot_findings(db::schema_before_marked_mechanical()))
        ))),
        BEFORE_DESIGN_VARIANTS_LEFT_STAMP
    );
}

/// A store from before the variants of a design change left the schema
/// loses their tables, with the variants they kept, keeps its design changes
/// and equals a fresh schema.
#[test]
fn a_store_from_before_the_design_variants_left_loses_their_tables() {
    let scratch = Scratch::new("move-design-variants-left");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(with_design_variants(
            db::with_review_texts(db::with_shot_findings(db::schema_before_marked_mechanical())),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_DESIGN_VARIANTS_LEFT_STAMP)
            .unwrap();
        use crate::lists::fixtures::v;
        crate::lists::project::plant(&conn, "repo", &[("name", v("bae"))]).unwrap();
        crate::lists::record::plant(
            &conn,
            "record",
            &[
                ("id", v(1)),
                ("kind", v("design")),
                ("opened_at", v(5)),
                ("opened_by", v("tester")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design",
            &[
                ("id", v(1)),
                ("title", v("People rows")),
                ("details", v("d")),
                ("repo", v("bae")),
                ("worktree", v("/w")),
            ],
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO design_variants (design_id, board, selector, theme, tip) VALUES (1, 'design/mocks/A.dc.html', 'p', 'dark', '{}');
INSERT INTO design_variant (design_id, position, props) VALUES (1, 0, '{{}}');",
            "a".repeat(40)
        ))
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let titles: Vec<String> = conn
        .prepare("SELECT title FROM design")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(titles, ["People rows"]);
}

#[test]
fn the_land_app_steps_stamp_is_the_schema_before_it() {
    assert_eq!(
        stamp_before_show_now(before_land_app_steps(db::with_review_texts(
            db::with_shot_findings(db::schema_before_marked_mechanical())
        ))),
        BEFORE_LAND_APP_STEPS_STAMP
    );
}

/// A landing of one step of <kind>.
fn landing(kind: crate::land_run::StepKind) -> crate::land_run::LandRun {
    crate::land_run::LandRun {
        project: "bae".to_string(),
        started_at: 5,
        repo: "bae".to_string(),
        worktree: "/w".to_string(),
        branch: None,
        by: "agent".to_string(),
        ms: 9,
        landed: true,
        items: Vec::new(),
        commits: Vec::new(),
        steps: vec![crate::land_run::Step {
            kind,
            started_at: 5,
            ms: 3,
        }],
    }
}

/// A store from before a landing's steps took the app wait and the app
/// build keeps each step and equals a fresh schema, whose check takes both.
#[test]
fn a_store_from_before_the_app_landing_steps_keeps_its_steps_and_takes_them() {
    let scratch = Scratch::new("move-land-app-steps");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_land_app_steps(
            db::with_review_texts(db::with_shot_findings(db::schema_before_marked_mechanical())),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_LAND_APP_STEPS_STAMP)
            .unwrap();
        crate::land_run::write(&mut conn, &landing(crate::land_run::StepKind::Checks)).unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let mut conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    crate::land_run::write(&mut conn, &landing(crate::land_run::StepKind::Fetch)).unwrap();
    let kinds: Vec<String> = conn
        .prepare("SELECT kind FROM land_step ORDER BY land_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(kinds, ["checks", "fetch"]);
}

#[test]
fn the_review_board_show_now_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&before_review_board_show_now(before_app_build_stopped(
            db::with_review_texts(db::with_shot_findings(db::schema_before_marked_mechanical()))
        ))),
        BEFORE_REVIEW_BOARD_SHOW_NOW_STAMP
    );
}

/// A store from before a review board said whether the page shows its now
/// keeps each board and its circles, and each board shows its now, as every
/// board's page did then; the store then equals a fresh schema.
#[test]
fn a_store_from_before_the_review_boards_show_now_keeps_each_board_with_its_now() {
    let scratch = Scratch::new("move-review-board-show-now");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(before_review_board_show_now(
            db::with_review_texts(db::with_shot_findings(db::schema_before_marked_mechanical())),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_REVIEW_BOARD_SHOW_NOW_STAMP)
            .unwrap();
        use crate::lists::fixtures::v;
        crate::lists::project::plant(&conn, "repo", &[("name", v("bae"))]).unwrap();
        crate::lists::record::plant(
            &conn,
            "record",
            &[
                ("id", v(1)),
                ("kind", v("design")),
                ("opened_at", v(5)),
                ("opened_by", v("tester")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design",
            &[
                ("id", v(1)),
                ("title", v("People rows")),
                ("details", v("d")),
                ("repo", v("bae")),
                ("worktree", v("/w")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "review_section",
            &[("design_id", v(1)), ("heading", v("Rows"))],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "review_board",
            &[
                ("design_id", v(1)),
                ("position", v(0)),
                ("board", v("design/mocks/A.dc.html")),
                ("caption", v("A")),
                ("theme", v("dark")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "review_circle",
            &[
                ("design_id", v(1)),
                ("board", v(0)),
                ("position", v(0)),
                ("selector", v(".row")),
            ],
        )
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let boards: Vec<(String, i64)> = conn
        .prepare("SELECT board, show_now FROM review_board")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(boards, [("design/mocks/A.dc.html".to_string(), 1)]);
    let circles: i64 = conn
        .query_row("SELECT count(*) FROM review_circle", [], |row| row.get(0))
        .unwrap();
    assert_eq!(circles, 1);
}

#[test]
fn the_app_build_stopped_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&before_app_build_stopped(db::with_review_texts(
            db::with_shot_findings(db::schema_before_marked_mechanical())
        ))),
        BEFORE_APP_BUILD_STOPPED_STAMP
    );
}

/// A store from before an app build run could end stopped keeps each run
/// and step: a run that passed says so, every other run failed. The store
/// then equals a fresh schema, which takes a stopped run and step.
#[test]
fn a_store_from_before_stopped_app_builds_keeps_each_run_with_how_it_ended() {
    let scratch = Scratch::new("move-app-build-stopped");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&before_app_build_stopped(db::with_review_texts(
            db::with_shot_findings(db::schema_before_marked_mechanical()),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_APP_BUILD_STOPPED_STAMP)
            .unwrap();
        conn.execute_batch(
            "INSERT INTO app_build_runs (id, started_at, repo, lane, checkout, built_ref, wait_ms, passed) VALUES (1, 5, 'bae', 'mac', 'side-1', 'one', 0, 1), (2, 6, 'bae', 'mac', 'side-1', 'two', 0, 0);
INSERT INTO app_build_steps (run_id, position, name, kind, started_at, wall_ms, cpu_ms, result) VALUES (1, 0, 'macos', 'step', 5, 1, 1, 'passed'), (2, 0, 'macos', 'step', 6, 1, 1, 'failed');",
        )
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let runs: Vec<(String, String)> = conn
        .prepare("SELECT built_ref, outcome FROM app_build_runs ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        runs,
        [
            ("one".to_string(), "passed".to_string()),
            ("two".to_string(), "failed".to_string())
        ]
    );
    let steps: i64 = conn
        .query_row("SELECT count(*) FROM app_build_steps", [], |row| row.get(0))
        .unwrap();
    assert_eq!(steps, 2);
    drop(conn);
    crate::app_timing::record(
        &path,
        &crate::app_timing::RunRecord {
            started_at: 7,
            repo: "bae".into(),
            lane: crate::lane::Lane::Mac,
            checkout: "side-1".into(),
            built_ref: "three".into(),
            built_commit: None,
            who: None,
            wait_ms: 0,
            outcome: crate::run_end::RunEnd::Stopped,
            load: None,
            steps: vec![crate::app_timing::StepRecord {
                at_merge_base: None,
                name: "macos".into(),
                kind: crate::app_timing::StepKind::Step,
                started_at: 7,
                wall_ms: 1,
                cpu_ms: 1,
                result: crate::app_timing::StepResult::Stopped,
                shots: None,
            }],
        },
    )
    .unwrap();
}

#[test]
fn the_shots_on_volume_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::with_review_texts(db::with_shot_findings(
            db::schema_before_marked_mechanical()
        ))),
        BEFORE_SHOTS_ON_VOLUME_STAMP
    );
}

/// A store from before builds kept the shots runs on the build volume loses
/// the shot findings' table and the folder of reports beside its database,
/// keeps everything else, and equals a fresh schema.
#[test]
fn a_store_from_before_the_shots_runs_left_it_loses_their_table_and_reports() {
    let scratch = Scratch::new("move-shots-on-volume");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    let reports = dir.join("shots/0868b3ff8702-mac-mac.-d4eee7325e0d/img");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(reports.join("a.png"), "png").unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::with_review_texts(db::with_shot_findings(
            db::schema_before_marked_mechanical(),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_SHOTS_ON_VOLUME_STAMP)
            .unwrap();
        conn.execute_batch(
            "INSERT INTO shot_findings (repo, commit_id, platform, prefix, shots_version, recorded_at, exit_code, findings) VALUES ('bae', 'c', 'mac', 'mac.', 'v', 5, 1, 'FAIL x');
INSERT INTO hook_runs (started_at, repo, worktree, passed, plan_ms, total_ms) VALUES (5, 'bae', '/w', 1, 0, 0);",
        )
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    assert!(!dir.join("shots").exists());
    let runs: i64 = conn
        .query_row("SELECT count(*) FROM hook_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(runs, 1);
}

/// The move gives the dropped table's pages back to the disk: the store's
/// file shrinks once the findings it held are gone.
#[test]
fn a_store_from_before_the_shots_runs_left_it_shrinks_by_what_their_table_held() {
    let scratch = Scratch::new("move-shots-on-volume-shrinks");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::with_review_texts(db::with_shot_findings(
            db::schema_before_marked_mechanical(),
        )))
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_SHOTS_ON_VOLUME_STAMP)
            .unwrap();
        conn.execute(
            "INSERT INTO shot_findings (repo, commit_id, platform, prefix, shots_version, recorded_at, exit_code, findings) VALUES ('bae', 'c', 'mac', 'mac.', 'v', 5, 1, ?1)",
            ["FAIL x\n".repeat(400_000)],
        )
        .unwrap();
    }
    let size = |path: &std::path::Path| std::fs::metadata(path).unwrap().len();
    let before = size(&path);
    assert!(before > 2_000_000, "{before}");
    migrate_stores(&places, 0).unwrap();
    assert!(size(&path) < 1_000_000, "{} after the move", size(&path));
}

#[test]
fn the_review_texts_on_volume_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::with_review_texts(db::schema_before_marked_mechanical())),
        BEFORE_REVIEW_TEXTS_ON_VOLUME_STAMP
    );
}

/// A store from before builds kept the review texts on the build volume
/// writes each run's texts there, holds the hash of each answer in place of
/// the answer, loses the texts' table, keeps every record, and equals a fresh
/// schema.
#[test]
fn a_store_from_before_the_review_texts_left_it_keeps_them_on_the_volume_and_holds_hashes() {
    use crate::review::texts::{Texts, hash};
    let scratch = Scratch::new("move-review-texts");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::with_review_texts(db::schema_before_marked_mechanical()))
            .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_REVIEW_TEXTS_ON_VOLUME_STAMP)
            .unwrap();
        crate::review::record::tests::plant_run_with_texts_in_the_store(
            &conn,
            (
                &hash("the prompt"),
                &hash("the system"),
                &hash("the message"),
            ),
        )
        .unwrap();
        crate::review::exchange_time::record(&conn, 6, 5, 7).unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let (prompt, output_hash): (String, String) = conn
        .query_row(
            "SELECT c.prompt, e.output_hash FROM review_exchange e JOIN review_call c ON c.id = e.call_id WHERE e.id = 6",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(output_hash, hash("the answer"));
    let texts = Texts::of(&places, "bae");
    for (hash, text) in [
        (prompt.as_str(), "the prompt"),
        (output_hash.as_str(), "the answer"),
    ] {
        assert_eq!(texts.read(4, hash).unwrap(), text);
    }
    let times: i64 = conn
        .query_row("SELECT count(*) FROM review_exchange_time", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(times, 1);
}

#[test]
fn the_marked_mechanical_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_marked_mechanical()),
        BEFORE_MARKED_MECHANICAL_STAMP
    );
}

/// A store from before the coordinator could mark a branch as mechanical
/// gains the empty table and equals a fresh schema.
#[test]
fn a_store_from_before_the_marked_mechanical_table_gains_it_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-marked-mechanical");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_marked_mechanical())
            .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_MARKED_MECHANICAL_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

/// The stamp the shots drops' move leaves is the schema before them.
#[test]
fn the_app_shots_drop_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_app_shots_drop()),
        BEFORE_APP_SHOTS_DROP_STAMP
    );
}

/// A store from before agents could drop a shots prefix from a landing gains
/// the empty table and equals a fresh schema.
#[test]
fn a_store_from_before_the_shots_drops_gains_the_table_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-shots-drop");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_app_shots_drop())
            .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_APP_SHOTS_DROP_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_ack_repo_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_ack_repo()),
        BEFORE_ACK_REPO_STAMP
    );
}

/// A store from before the acknowledgements stopped being keyed by repo
/// loses them, keeps the table's new shape and equals a fresh schema.
#[test]
fn a_store_from_before_the_acks_left_the_repo_loses_them_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-ack-repo");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_ack_repo()).unwrap();
        crate::ack::record::tests::plant_acks_keyed_by_repo(&conn).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_ACK_REPO_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM ack", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn the_ack_tokens_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_ack_tokens()),
        BEFORE_ACK_TOKENS_STAMP
    );
}

/// A store from before the readings were chained by tokens loses its readings
/// and their made-up words, keeps its acknowledgements and equals a fresh
/// schema.
#[test]
fn a_store_from_before_the_ack_tokens_loses_its_readings_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-ack-tokens");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_ack_tokens()).unwrap();
        crate::ack::record::tests::plant_before_tokens(&conn).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_ACK_TOKENS_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    for (table, expected) in [("ack", 1), ("ack_reading", 0)] {
        let rows: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, expected, "{table}");
    }
}

#[test]
fn the_ack_unread_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_ack_unread()),
        BEFORE_ACK_UNREAD_STAMP
    );
}

/// A store from before nothing read the time of an acknowledgement or the
/// sections of a reading keeps its acknowledgements and readings, loses the
/// time and the sections' table, and equals a fresh schema.
#[test]
fn a_store_from_before_the_unread_columns_keeps_its_acks_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-ack-unread");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_ack_unread()).unwrap();
        crate::ack::record::tests::plant_before_unread(&conn).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_ACK_UNREAD_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let version: String = conn
        .query_row("SELECT version FROM ack", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, "v1");
    for table in ["ack_reading", "ack_token"] {
        let rows: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 1, "{table}");
    }
}

#[test]
fn the_review_component_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_review_component()),
        BEFORE_REVIEW_COMPONENT_STAMP
    );
}

/// A store from before a review page's entry could be a component's preview
/// keeps each entry as a board with its theme and its circles, and equals a
/// fresh schema.
#[test]
fn a_store_from_before_review_components_keeps_each_entry_as_a_board() {
    let scratch = Scratch::new("move-review-component");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_review_component())
            .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_REVIEW_COMPONENT_STAMP)
            .unwrap();
        use crate::lists::fixtures::v;
        crate::lists::project::plant(&conn, "repo", &[("name", v("bae"))]).unwrap();
        crate::lists::record::plant(
            &conn,
            "record",
            &[
                ("id", v(1)),
                ("kind", v("design")),
                ("opened_at", v(5)),
                ("opened_by", v("tester")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design",
            &[
                ("id", v(1)),
                ("title", v("People rows")),
                ("details", v("d")),
                ("repo", v("bae")),
                ("worktree", v("/w")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "review_section",
            &[("design_id", v(1)), ("heading", v("Rows"))],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "review_board",
            &[
                ("design_id", v(1)),
                ("position", v(0)),
                ("board", v("design/mocks/A.dc.html")),
                ("caption", v("A")),
                ("theme", v("light")),
                ("show_now", v(0)),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "review_circle",
            &[
                ("design_id", v(1)),
                ("board", v(0)),
                ("position", v(0)),
                ("selector", v(".row")),
            ],
        )
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let entries: Vec<String> = conn
        .prepare("SELECT board || ' ' || ifnull(component, 'no component') || ' ' || theme || ' ' || show_now FROM review_board")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(entries, ["design/mocks/A.dc.html no component light 0"]);
    let circles: i64 = conn
        .query_row("SELECT count(*) FROM review_circle", [], |row| row.get(0))
        .unwrap();
    assert_eq!(circles, 1);
}

#[test]
fn the_deleted_component_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_deleted_place()),
        BEFORE_DELETED_COMPONENT_STAMP
    );
}

/// A store from before a design change could delete a component keeps each
/// deleted board, accepts a deleted component's folder, and equals a fresh
/// schema.
#[test]
fn a_store_from_before_deleted_components_keeps_each_deleted_board() {
    let scratch = Scratch::new("move-deleted-component");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_deleted_place())
            .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_DELETED_COMPONENT_STAMP)
            .unwrap();
        use crate::lists::fixtures::v;
        crate::lists::project::plant(&conn, "repo", &[("name", v("bae"))]).unwrap();
        crate::lists::record::plant(
            &conn,
            "record",
            &[
                ("id", v(1)),
                ("kind", v("design")),
                ("opened_at", v(5)),
                ("opened_by", v("tester")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design",
            &[
                ("id", v(1)),
                ("title", v("Old board")),
                ("details", v("d")),
                ("repo", v("bae")),
                ("worktree", v("/w")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design_deleted",
            &[("design_id", v(1)), ("path", v("design/mocks/Old.dc.html"))],
        )
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    {
        use crate::lists::fixtures::v;
        crate::lists::design::plant(
            &conn,
            "design_deleted",
            &[
                ("design_id", v(1)),
                ("path", v("design/system/components/Gone/")),
            ],
        )
        .unwrap();
    }
    let kept: Vec<String> = conn
        .prepare("SELECT path FROM design_deleted ORDER BY path")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        kept,
        ["design/mocks/Old.dc.html", "design/system/components/Gone/"]
    );
}

#[test]
fn the_app_build_at_merge_base_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_app_build_at_merge_base()),
        BEFORE_APP_BUILD_AT_MERGE_BASE_STAMP
    );
}

/// A store from before builds recorded where each step ran keeps each step
/// with no flag, since builds never recorded which it was, and equals a fresh
/// schema.
#[test]
fn a_store_from_before_the_merge_base_flag_keeps_each_step_with_no_flag_and_equals_a_fresh_schema()
{
    let scratch = Scratch::new("move-at-merge-base");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_app_build_at_merge_base())
            .unwrap();
        conn.execute_batch(
            "INSERT INTO app_build_runs (id, started_at, repo, lane, checkout, built_ref, wait_ms, outcome) VALUES (1, 5, 'bae', 'mac', 'side-1', 'one', 0, 'passed');
             INSERT INTO app_build_steps (run_id, position, name, kind, started_at, wall_ms, cpu_ms, result) VALUES (1, 0, 'macos', 'step', 5, 1, 1, 'passed')",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_APP_BUILD_AT_MERGE_BASE_STAMP)
            .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    let flag: Option<bool> = conn
        .query_row("SELECT at_merge_base FROM app_build_steps", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(flag, None);
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
}

#[test]
fn the_app_build_step_shots_stamp_is_the_schema_before_it() {
    assert_eq!(
        db::stamp_of_text(&db::schema_before_app_build_step_shots()),
        BEFORE_APP_BUILD_STEP_SHOTS_STAMP
    );
}

/// A store from before the shots steps kept what they used keeps its steps,
/// gains the empty table and equals a fresh schema.
#[test]
fn a_store_from_before_the_shots_uses_keeps_its_steps_and_gains_the_table() {
    let scratch = Scratch::new("move-step-shots");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&db::schema_before_app_build_step_shots())
            .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_APP_BUILD_STEP_SHOTS_STAMP)
            .unwrap();
        conn.execute_batch(
            "INSERT INTO app_build_runs (id, started_at, repo, lane, checkout, built_ref, wait_ms, outcome) VALUES (1, 5, 'bae', 'mac', 'side-1', 'b', 0, 'passed');
             INSERT INTO app_build_steps (run_id, position, name, kind, started_at, wall_ms, cpu_ms, result) VALUES (1, 0, 'shots mac.a.', 'shots', 5, 1, 1, 'passed');",
        )
        .unwrap();
    }
    migrate_stores(&places, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let steps: i64 = conn
        .query_row("SELECT count(*) FROM app_build_steps", [], |row| row.get(0))
        .unwrap();
    let uses: i64 = conn
        .query_row("SELECT count(*) FROM app_build_step_shots", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!((steps, uses), (1, 0));
}

#[test]
fn a_move_allows_unknown_spec_cost_and_keeps_previous_checks_and_findings() {
    use crate::lists::{fixtures, project, spec_check};
    use crate::review::call::Usage;
    let scratch = Scratch::new("move-spec-check-cost");
    let places = scratch.places();
    let path = scratch.dir.join(DATABASE);
    let item = {
        let conn = Connection::open(&path).unwrap();
        let prior = db::schema_before_unreported_usage();
        assert_eq!(db::stamp_of_text(&prior), BEFORE_UNREPORTED_USAGE_STAMP);
        conn.execute_batch(&prior).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_UNREPORTED_USAGE_STAMP)
            .unwrap();
        project::init(
            &conn,
            &project::NewProject {
                name: "demo".into(),
                canvas: true,
                repos: vec!["bae".into()],
                places: Default::default(),
                sources: Default::default(),
                specs: Default::default(),
            },
        )
        .unwrap();
        crate::lists::area::add_area(&conn, "Alpha", None).unwrap();
        let item = fixtures::item(&conn, Vec::new());
        spec_check::tests::plant_before_unreported_usage(&conn, item, "opus", None);
        spec_check::tests::plant_before_unreported_usage(&conn, item, "gpt-6.2-sol", None);
        spec_check::tests::plant_before_unreported_usage(
            &conn,
            item,
            "opus",
            Some("bin/custom-reviewer exited with exit status: 1: limit"),
        );
        item
    };
    move_store(&places, &path, 0).unwrap();
    let conn = db::open(&path, db::Make::Never).unwrap();
    spec_check::tests::record_one(&conn, item, "gpt-6.2-astra", Usage::default());
    assert_eq!(
        spec_check::tests::recorded_usage(&conn),
        [
            (Some(0), Some(0)),
            (Some(0), None),
            (None, None),
            (None, None)
        ]
    );
    let check = &spec_check::read_all(&conn).unwrap()[&item];
    assert_eq!(check.len(), 4);
    assert!(
        check
            .iter()
            .all(|check| check.conflicts == 1 && check.unstated == 1)
    );
}

#[test]
fn adding_codex_destinations_keeps_registered_claude_inboxes() {
    let old = db::schema_before_codex_inboxes();
    assert_eq!(db::stamp_of_text(&old), BEFORE_CODEX_INBOX_STAMP);
    let scratch = Scratch::new("move-codex-inbox");
    let places = scratch.places();
    let dir = places.lists.join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(&old).unwrap();
        db.pragma_update(None, "user_version", BEFORE_CODEX_INBOX_STAMP)
            .unwrap();
        db.execute_batch("INSERT INTO design_listener VALUES (1, '/tmp/listener.sock', 'design-secret', 10, 'designer');
                          INSERT INTO review_judge VALUES (1, '/tmp/judge.sock', 'judge-secret', 'judge-id', 11, 'judge');").unwrap();
    }
    migrate_stores(&places, 12).unwrap();
    let db = db::open(&path, db::Make::Never).unwrap();
    for (table, expected) in [
        (
            "design_listener",
            ("claude", "/tmp/listener.sock", "design-secret"),
        ),
        (
            "review_judge",
            ("claude", "/tmp/judge.sock", "judge-secret"),
        ),
    ] {
        let stored: (String, String, String) = db
            .query_row(
                &format!("SELECT runtime, address, token FROM {table}"),
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (stored.0.as_str(), stored.1.as_str(), stored.2.as_str()),
            expected
        );
    }
}
