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

/// The column that names who ends a condition, as item.sql writes it.
const CONDITION_ENDER_COLUMN: &str = "    until TEXT NOT NULL CONSTRAINT item_wait_condition_ender_known CHECK (until IN ('agent', 'coordinator', 'ci')),\n";

/// The schema text before a condition named who ends it: the schema before
/// the item events without the ender's column.
fn schema_before_condition_enders() -> String {
    let schema = before_landing_repo(db::schema_before_item_event());
    assert_eq!(schema.matches(CONDITION_ENDER_COLUMN).count(), 1);
    schema.replacen(CONDITION_ENDER_COLUMN, "", 1)
}

/// Creates a store in <dir> with the schema from before the judge: that
/// schema's text and its stamp. Install moves it to today's schema.
pub fn store_before_judge(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&before_landing_repo(db::schema_before_judge()))
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
        db::stamp_of_text(&before_landing_repo(db::schema_before_item_event())),
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
        conn.execute_batch(&before_landing_repo(db::schema_before_item_event()))
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
        db::stamp_of_text(&before_landing_repo(db::schema_before_land_run())),
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
        conn.execute_batch(&before_landing_repo(db::schema_before_land_run()))
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
        db::stamp_of_text(&before_landing_repo(db::schema_before_exchange_time())),
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
        conn.execute_batch(&before_landing_repo(db::schema_before_exchange_time()))
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
        db::stamp_of_text(&before_landing_repo(db::schema_before_host_sample())),
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
        conn.execute_batch(&before_landing_repo(db::schema_before_host_sample()))
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
        db::stamp_of_text(&before_landing_repo(db::schema_before_cargo_run_crate())),
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
        conn.execute_batch(&before_landing_repo(db::schema_before_cargo_run_crate()))
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
        db::stamp_of_text(&schema_before_condition_enders()),
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
        conn.execute_batch(&schema_before_condition_enders())
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
        db::stamp_of_text(&before_landing_repo(db::schema_before_judge())),
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
        conn.execute_batch(&before_landing_repo(db::schema_before_judge()))
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
        db::stamp_of_text(&before_landing_repo(db::schema_before_card_state())),
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
    conn.execute_batch(&before_landing_repo(db::schema_before_card_state()))
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
        db::stamp_of_text(&before_review_text_rowid(db::schema_before_card_state())),
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
        conn.execute_batch(&before_review_text_rowid(db::schema_before_card_state()))
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
        db::stamp_of_text(&before_review_text_rowid(
            db::schema_before_cargo_run_outcome()
        )),
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
        conn.execute_batch(&before_review_text_rowid(
            db::schema_before_cargo_run_outcome(),
        ))
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
        db::stamp_of_text(&before_review_text_rowid(db::schema_before_step_commits())),
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
        conn.execute_batch(&before_review_text_rowid(db::schema_before_step_commits()))
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
        db::stamp_of_text(&before_review_text_rowid(db::schema())),
        BEFORE_REVIEW_TEXT_ROWID_STAMP
    );
}
