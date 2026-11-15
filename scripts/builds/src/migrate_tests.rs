use super::*;
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

/// The one hook run the store below holds.
fn plant_run(conn: &Connection) {
    crate::hook_stats::plant(
        conn,
        "hook_runs",
        &[
            ("started_at", 5_i64.into()),
            ("repo", "bae".to_string().into()),
            ("worktree", "/d/bae".to_string().into()),
            ("passed", 1_i64.into()),
            ("plan_ms", 1_i64.into()),
            ("total_ms", 9_i64.into()),
        ],
    )
    .unwrap();
}

/// The schema's text before `design_tip`: today's without that table and the
/// comment above it.
fn schema_before_design_tip() -> String {
    let schema = without_design_fix(db::schema_before_app_timing());
    let from = schema
        .find("\n-- The last commit of the range")
        .expect("the table's comment");
    let end = "STRICT;\n";
    let table = from + schema[from..].find("CREATE TABLE design_tip").unwrap();
    let to = table + schema[table..].find(end).unwrap() + end.len();
    format!("{}{}", &schema[..from], &schema[to..])
}

/// The schema's text before `hook_run_checks` kept a time only for a passed
/// check: the one before `design_tip` with the older comment and the older
/// rule for `ms`.
fn schema_before_check_times() -> String {
    let schema = schema_before_design_tip();
    let from = schema
        .find("    -- What it was handed (NULL for a skipped one)")
        .expect("the column's comment");
    let to = from + schema[from..].find("    files INTEGER").unwrap();
    let older = "    -- What it was handed, and how long it took: NULL for a skipped one.\n";
    format!("{}{older}{}", &schema[..from], &schema[to..]).replace(
        "CHECK ((outcome = 'passed') = (ms IS NOT NULL))",
        "CHECK ((outcome = 'skipped') = (ms IS NULL))",
    )
}

/// The schema's text before `design_deleted`: the one before check times
/// without that table and the comment above it.
fn schema_before_design_deleted() -> String {
    let schema = schema_before_check_times();
    let from = schema
        .find("\n-- A board the change deletes")
        .expect("the table's comment");
    let table = from + schema[from..].find("CREATE TABLE design_deleted").unwrap();
    let end = "WITHOUT ROWID;\n";
    let to = table + schema[table..].find(end).unwrap() + end.len();
    format!("{}{}", &schema[..from], &schema[to..])
}

/// A store as the schema before `design_deleted` made it: that schema's text, its stamp, one run.
fn store_before_design_deleted(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&schema_before_design_deleted()).unwrap();
    conn.pragma_update(None, "user_version", BEFORE_DESIGN_DELETED_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_of_the_schema_before_deleted_boards_gains_the_table_keeps_its_runs_and_equals_a_fresh_schema()
 {
    let scratch = Scratch::new("move-deleted");
    let places = scratch.places();
    let path = store_before_design_deleted(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
    // A second install changes nothing.
    let before = db::schema_rows(&conn).unwrap();
    migrate_stores(&places).unwrap();
    assert_eq!(
        db::schema_rows(&Connection::open(&path).unwrap()).unwrap(),
        before
    );
    assert_eq!(
        db::stamp_of(&Connection::open(&path).unwrap(), &path).unwrap(),
        db::stamp()
    );
}

#[test]
fn a_store_with_a_stamp_the_deleted_boards_move_does_not_know_is_refused_and_left_as_it_was() {
    let scratch = Scratch::new("move-deleted-unknown");
    let places = scratch.places();
    let path = store_before_design_deleted(&places.lists.join("bae"));
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 1234)
        .unwrap();
    let refused = migrate_stores(&places).unwrap_err().text();
    assert!(
        refused.contains("stamp 1234") && refused.contains("does not know"),
        "{refused}"
    );
    let conn = Connection::open(&path).unwrap();
    assert!(!has_design_deleted(&conn));
}

#[test]
fn a_deleted_boards_move_that_would_not_equal_a_fresh_schema_rolls_back() {
    let scratch = Scratch::new("move-deleted-drift");
    let places = scratch.places();
    let path = store_before_design_deleted(&places.lists.join("bae"));
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP INDEX idx_hook_runs_started")
        .unwrap();
    let refused = migrate_stores(&places).unwrap_err().text();
    assert!(
        refused.contains("not the one this builds makes"),
        "{refused}"
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        (
            has_design_deleted(&conn),
            db::stamp_of(&conn, &path).unwrap()
        ),
        (false, BEFORE_DESIGN_DELETED_STAMP)
    );
}

fn has_design_deleted(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'design_deleted'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

#[test]
fn the_previous_stamp_is_the_schema_without_the_deleted_boards_table() {
    let text = schema_before_design_deleted();
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_DESIGN_DELETED_STAMP
    );
}

/// A store as the schema before check times made it: that schema's text, its stamp, one run with
/// a passed check and a failed one, each with a time as the old schema kept it.
fn store_before_check_times(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&schema_before_check_times()).unwrap();
    conn.pragma_update(None, "user_version", BEFORE_CHECK_TIMES_STAMP)
        .unwrap();
    plant_run(&conn);
    conn.execute_batch(
        "INSERT INTO hook_run_checks (run_id, position, name, kind, outcome, files, crates, ms) VALUES
            (1, 0, 'fmt', 'check', 'passed', 1, 0, 40),
            (1, 1, 'clippy', 'check', 'failed', 1, 3, 90),
            (1, 2, 'docs', 'check', 'skipped', NULL, NULL, NULL)",
    )
    .unwrap();
    path
}

#[test]
fn a_store_from_before_check_times_loses_a_failed_checks_time_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-check-times");
    let places = scratch.places();
    let path = store_before_check_times(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let times: Vec<(String, Option<i64>)> = conn
        .prepare("SELECT name, ms FROM hook_run_checks ORDER BY position")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        times,
        vec![
            ("fmt".to_string(), Some(40)),
            ("clippy".to_string(), None),
            ("docs".to_string(), None)
        ]
    );
}

#[test]
fn the_check_times_stamp_is_the_schema_before_check_times() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in schema_before_check_times().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_CHECK_TIMES_STAMP
    );
}

/// A store as the schema before `design_tip` made it: that schema's text, its stamp, one run.
fn store_before_design_tip(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&schema_before_design_tip()).unwrap();
    conn.pragma_update(None, "user_version", BEFORE_DESIGN_TIP_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_of_the_schema_before_design_tips_gains_the_table_keeps_its_runs_and_equals_a_fresh_schema()
 {
    let scratch = Scratch::new("move-tip");
    let places = scratch.places();
    let path = store_before_design_tip(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
    assert_eq!(
        db::stamp_of(&Connection::open(&path).unwrap(), &path).unwrap(),
        db::stamp()
    );
}

#[test]
fn the_design_tip_stamp_is_the_schema_before_design_tips() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in schema_before_design_tip().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_DESIGN_TIP_STAMP
    );
}

/// A store as the schema before the app-build timings made it: that schema's text, its stamp,
/// one run.
fn store_before_app_timing(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(db::schema_before_app_timing()))
        .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_APP_TIMING_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_from_before_app_timings_gains_their_tables_keeps_its_runs_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-app-timing");
    let places = scratch.places();
    let path = store_before_app_timing(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
}

#[test]
fn the_app_timing_stamp_is_the_schema_before_app_timings() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in without_design_fix(db::schema_before_app_timing()).bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_APP_TIMING_STAMP
    );
}

/// A store as the schema before the design sessions' table made it: that
/// schema's text, its stamp, one run.
fn store_before_design_session(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(db::schema_before_design_session()))
        .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_DESIGN_SESSION_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_from_before_design_sessions_gains_their_table_keeps_its_runs_and_equals_a_fresh_schema()
{
    let scratch = Scratch::new("move-design-session");
    let places = scratch.places();
    let path = store_before_design_session(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
}

#[test]
fn the_design_session_stamp_is_the_schema_before_design_sessions() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in without_design_fix(db::schema_before_design_session()).bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_DESIGN_SESSION_STAMP
    );
}

/// A store as the schema before the shot findings' table made it: that
/// schema's text, its stamp, one run.
fn store_before_shot_findings(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(db::schema_before_shot_findings()))
        .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_SHOT_FINDINGS_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_from_before_shot_findings_gains_their_table_keeps_its_runs_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-shot-findings");
    let places = scratch.places();
    let path = store_before_shot_findings(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
}

#[test]
fn the_shot_findings_stamp_is_the_schema_before_shot_findings() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in without_design_fix(db::schema_before_shot_findings()).bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_SHOT_FINDINGS_STAMP
    );
}

/// A store as the schema before the review's tables made it: that schema's
/// text, its stamp, one run.
fn store_before_review(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(db::schema_before_review()))
        .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_REVIEW_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_from_before_the_review_gains_its_tables_keeps_its_runs_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-review");
    let places = scratch.places();
    let path = store_before_review(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN ('review_landing', 'review_run', 'review_call', 'review_text', 'review_exchange', 'review_verdict', 'review_finding')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 7);
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
}

#[test]
fn the_review_stamp_is_the_schema_before_the_review() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in without_design_fix(db::schema_before_review()).bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_REVIEW_STAMP
    );
}

/// A store as the schema before the cargo runs' table made it: that schema's
/// text, its stamp, one run.
fn store_before_cargo_runs(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(db::schema_before_cargo_runs()))
        .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_CARGO_RUNS_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_from_before_cargo_runs_gains_their_table_keeps_its_runs_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-cargo-runs");
    let places = scratch.places();
    let path = store_before_cargo_runs(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
}

#[test]
fn the_cargo_runs_stamp_is_the_schema_before_cargo_runs() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in without_design_fix(db::schema_before_cargo_runs()).bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_CARGO_RUNS_STAMP
    );
}

/// The schema text before the design fixes' table: today's, without the table
/// and with the approval's comment as it read then.
fn schema_before_design_fix() -> String {
    without_design_fix(db::schema())
}

/// <schema> as the text read before reviews ran before landing: the review's
/// header, `review_landing` and the audit's words as they read then, and no
/// `review_commit` or `review_fix`. A schema without the review's tables is
/// left as it is.
fn without_landing_review(schema: String) -> String {
    let added = "\n-- The commits a review read, in order,";
    let Some(start) = schema.find(added) else {
        return schema;
    };
    let end = "CHECK (trim(by) <> '')\n) STRICT;\n";
    let stop = start + schema[start..].find(end).expect("it ends") + end.len();
    let mut old = format!("{}{}", &schema[..start], &schema[stop..]);
    for (now, then) in [
        (
            "-- Every review of a change against the rules (`builds review`): the landing it
-- belongs to, each run and the commits it read, each reviewer call, its verdict
-- for every rule, each finding with how the coordinator settled it, and the
-- commit that fixed a confirmed finding. record.rs owns these tables and is the
-- only code that writes them.
",
            "-- Every review of a change against the rules (`builds review`): the landing it
-- belongs to, each run, each reviewer call, its verdict for every rule, and
-- each finding with how the coordinator settled it. record.rs owns these
-- tables and is the only code that writes them.
",
        ),
        (
            "-- A change under review: every review of it shares this landing, found by its
-- key, until the change lands. landed_at is when `builds land` pushed it, and
-- landed_run the run that covered the commits it pushed.
",
            "-- A landing under review: the reviews of one landing share a key until one of
-- them passes (closed_at).
",
        ),
        (
            "    landed_at INTEGER,
    landed_run INTEGER REFERENCES review_run (id),
    CONSTRAINT review_landing_landed_whole CHECK ((landed_at IS NULL) = (landed_run IS NULL))
",
            "    closed_at INTEGER
",
        ),
        (
            "-- of a landing that landed.\n",
            "-- of a landing that passed.\n",
        ),
    ] {
        assert_eq!(old.matches(now).count(), 1, "{now}");
        old = old.replace(now, then);
    }
    old
}

/// <schema> as the text read before the design fixes' table: every older
/// schema these tests rebuild is today's text with later parts taken out.
fn without_design_fix(schema: String) -> String {
    let schema = without_landing_review(schema);
    let fix = "-- A change that changes no design:";
    let start = schema.find(fix).expect("the schema has it");
    let stop = start + schema[start..].find("STRICT;\n\n").expect("it ends") + "STRICT;\n\n".len();
    let without = format!("{}{}", &schema[..start], &schema[stop..]);
    without.replace(
        "-- What lets the change land: the person's review, or the recording of a fix\n-- (design_fix). An amend deletes it.\n",
        "-- The person's review; an amend deletes it.\n",
    )
}

fn store_before_design_fix(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&schema_before_design_fix()).unwrap();
    conn.pragma_update(None, "user_version", BEFORE_DESIGN_FIX_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_from_before_design_fixes_gains_their_table_keeps_its_runs_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-design-fix");
    let places = scratch.places();
    let path = store_before_design_fix(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
}

#[test]
fn the_design_fix_stamp_is_the_schema_before_design_fixes() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in schema_before_design_fix().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_DESIGN_FIX_STAMP
    );
}

/// The schema text before reviews ran before landing.
fn schema_before_landing_review() -> String {
    without_landing_review(db::schema())
}

/// A store as the schema before reviews ran before landing made it, with one
/// landing whose review passed and one whose review did not.
fn store_before_landing_review(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&schema_before_landing_review()).unwrap();
    conn.pragma_update(None, "user_version", BEFORE_LANDING_REVIEW_STAMP)
        .unwrap();
    plant_run(&conn);
    crate::lists::project::plant(&conn, "repo", &[("name", "bae".to_string().into())]).unwrap();
    let landing = |id: i64, key: &str, opened: i64, closed: Option<i64>| {
        crate::review::record::plant(
            &conn,
            "review_landing",
            &[
                ("id", id.into()),
                ("key", key.to_string().into()),
                ("repo", "bae".to_string().into()),
                ("opened_at", opened.into()),
                ("closed_at", closed.into()),
            ],
        )
        .unwrap();
    };
    landing(1, "items:7", 5, Some(9));
    landing(2, "items:8", 6, None);
    for (id, landing, kind, at) in [
        (10, 1, "review", 6),
        (11, 1, "review", 8),
        (12, 1, "audit", 10),
        (13, 2, "review", 7),
    ] {
        crate::review::record::plant(
            &conn,
            "review_run",
            &[
                ("id", (id as i64).into()),
                ("landing_id", (landing as i64).into()),
                ("kind", kind.to_string().into()),
                ("at", (at as i64).into()),
                ("base", "b".to_string().into()),
                ("head", "h".to_string().into()),
                ("grouping", "g".to_string().into()),
                ("rules_version", String::new().into()),
            ],
        )
        .unwrap();
    }
    path
}

#[test]
fn a_store_from_before_landing_reviews_keeps_its_landings_with_the_run_each_passed_with() {
    let scratch = Scratch::new("move-landing-review");
    let places = scratch.places();
    let path = store_before_landing_review(&places.lists.join("bae"));
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let landings: Vec<(i64, Option<i64>, Option<i64>)> = conn
        .prepare("SELECT id, landed_at, landed_run FROM review_landing ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(landings, [(1, Some(9), Some(11)), (2, None, None)]);
    let runs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_run WHERE landing_id IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(runs, 4, "the runs keep their landings");
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
}

#[test]
fn the_landing_review_stamp_is_the_schema_before_landing_reviews() {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in schema_before_landing_review().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_LANDING_REVIEW_STAMP
    );
}
