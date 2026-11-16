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
    assert_eq!(db::stamp_of_text(&text), BEFORE_DESIGN_DELETED_STAMP);
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
    assert_eq!(
        db::stamp_of_text(&schema_before_check_times()),
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
    assert_eq!(
        db::stamp_of_text(&schema_before_design_tip()),
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
    assert_eq!(
        db::stamp_of_text(&without_design_fix(db::schema_before_app_timing())),
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
    assert_eq!(
        db::stamp_of_text(&without_design_fix(db::schema_before_design_session())),
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
    assert_eq!(
        db::stamp_of_text(&without_design_fix(db::schema_before_shot_findings())),
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
    assert_eq!(
        db::stamp_of_text(&without_design_fix(db::schema_before_review())),
        BEFORE_REVIEW_STAMP
    );
}

/// A store as the schema before the cargo runs' table made it: that schema's
/// text, its stamp, one run.
fn store_before_cargo_runs(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(db::schema()))
        .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_CARGO_RUNS_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_from_before_cargo_runs_keeps_its_runs_and_equals_a_fresh_schema() {
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
    assert_eq!(
        db::stamp_of_text(&without_design_fix(db::schema())),
        BEFORE_CARGO_RUNS_STAMP
    );
}

/// The schema text before the design fixes' table: today's, without the table
/// and with the approval's comment as it read then.
fn schema_before_design_fix() -> String {
    without_design_fix(schema_with_cargo_runs())
}

/// <schema> as the text read before reviews ran before landing: the review's
/// header, `review_landing` and the audit's words as they read then, and no
/// `review_commit` or `review_fix`. A schema without the review's tables is
/// left as it is.
fn without_landing_review(schema: String) -> String {
    let schema = without_scored_calls(schema);
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
    assert_eq!(
        db::stamp_of_text(&schema_before_design_fix()),
        BEFORE_DESIGN_FIX_STAMP
    );
}

/// The schema text before reviews ran before landing.
fn schema_before_landing_review() -> String {
    without_landing_review(schema_with_cargo_runs())
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
    assert_eq!(
        db::stamp_of_text(&schema_before_landing_review()),
        BEFORE_LANDING_REVIEW_STAMP
    );
}

/// <schema> as the text read before each review call kept its prompt and
/// cost: `review_call` without both columns and `review_text`'s comment as it
/// read then. A schema without the review's tables is left as it is.
fn without_scored_calls(schema: String) -> String {
    let schema = without_tags(schema);
    if !schema.contains("CREATE TABLE review_call") {
        return schema;
    }
    let mut old = schema;
    for (now, then) in [
        (
            "    -- The group's prompt without what each review brings (the change, the
    -- project's instructions and the brief): the system prompt's opening, the
    -- group's question, how to answer, its rules' text, its passes and the
    -- answer schema (prompt::setup). Calls with the same model and prompt ran
    -- the same setup.
    prompt TEXT NOT NULL REFERENCES review_text (hash),
",
            "",
        ),
        (
            "    -- What `claude -p` reported the call cost, in millionths of a US dollar,
    -- summed over its processes; null for a call made before builds recorded
    -- the cost.
    cost_microdollars INTEGER CONSTRAINT review_call_cost_not_negative CHECK (cost_microdollars >= 0),
",
            "",
        ),
        (
            "-- The texts a reviewer was sent (each exchange's system prompt and message)
-- and each call's prompt, kept once each by its hash (an FNV-1a hash and the
-- length).
",
            "-- The text a reviewer was sent and the text it answered with, kept once each
-- by its hash (an FNV-1a hash and the length).
",
        ),
    ] {
        assert_eq!(old.matches(now).count(), 1, "{now}");
        old = old.replace(now, then);
    }
    old
}

/// The schema text before each review call kept its prompt and cost.
fn schema_before_scored_calls() -> String {
    without_scored_calls(schema_with_cargo_runs())
}

/// The system prompt a call of group <group> was sent, with the project's
/// instructions before the group's part.
fn system_of(group: &str) -> String {
    format!(
        "You review one change.\n\n# Project instructions\n\nFacts.\n\n---\n\nGroup: {group}\nQuestion: q\n\n# Rules\n\n## r\n\nRule text.\n"
    )
}

/// A store as the schema before calls kept their prompt and cost made it, with
/// one review whose call made two passes over a file and the commit messages,
/// and one audit call with a single exchange, or none when not <audit_asked>.
fn store_before_scored_calls(dir: &std::path::Path, audit_asked: bool) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&schema_before_scored_calls()).unwrap();
    conn.pragma_update(None, "user_version", BEFORE_SCORED_CALLS_STAMP)
        .unwrap();
    plant_run(&conn);
    crate::lists::project::plant(&conn, "repo", &[("name", "bae".to_string().into())]).unwrap();
    let plant = |table: &str, columns: &[(&str, rusqlite::types::Value)]| {
        crate::review::record::plant(&conn, table, columns).unwrap();
    };
    let text = |s: &str| -> rusqlite::types::Value { s.to_string().into() };
    plant(
        "review_landing",
        &[
            ("id", 1_i64.into()),
            ("key", text("branch:work")),
            ("repo", text("bae")),
            ("opened_at", 5_i64.into()),
        ],
    );
    for (id, kind) in [(10_i64, "review"), (11, "audit")] {
        plant(
            "review_run",
            &[
                ("id", id.into()),
                ("landing_id", 1_i64.into()),
                ("kind", text(kind)),
                ("at", 6_i64.into()),
                ("base", text("b")),
                ("head", text("h")),
                ("grouping", text("g")),
                ("rules_version", text("")),
            ],
        );
    }
    for (id, run, group) in [(20_i64, 10_i64, "words"), (21, 11, "audit r")] {
        plant(
            "review_call",
            &[
                ("id", id.into()),
                ("run_id", run.into()),
                ("group_name", text(group)),
                ("model", text("opus")),
                ("rules", text("[\"r\"]")),
                ("tokens_in", 100_i64.into()),
                ("tokens_out", 10_i64.into()),
                ("tokens_cached", 0_i64.into()),
                ("tokens_written", 0_i64.into()),
            ],
        );
    }
    for (hash, body) in [
        ("s-words", system_of("words")),
        ("s-audit", system_of("audit r")),
        ("m", "the diff".to_string()),
    ] {
        plant(
            "review_text",
            &[("hash", text(hash)), ("text", body.into())],
        );
    }
    for (id, call, unit, pass, system) in [
        (30_i64, 20_i64, "a.rs", 0_i64, "s-words"),
        (31, 20, "COMMIT_MESSAGE", 0, "s-words"),
        (32, 20, "a.rs", 1, "s-words"),
        (33, 20, "COMMIT_MESSAGE", 1, "s-words"),
        (34, 21, "", 0, "s-audit"),
    ]
    .into_iter()
    .filter(|(_, call, ..)| audit_asked || *call != 21)
    {
        plant(
            "review_exchange",
            &[
                ("id", id.into()),
                ("call_id", call.into()),
                ("model", text("opus")),
                ("unit", text(unit)),
                ("pass", pass.into()),
                ("system_hash", text(system)),
                ("message_hash", text("m")),
                ("output", text("{}")),
                ("tokens_in", 25_i64.into()),
                ("tokens_out", 2_i64.into()),
                ("tokens_cached", 0_i64.into()),
                ("tokens_written", 0_i64.into()),
            ],
        );
    }
    plant(
        "review_verdict",
        &[
            ("call_id", 20_i64.into()),
            ("rule", text("r")),
            ("verdict", text("none")),
        ],
    );
    path
}

#[test]
fn a_store_from_before_scored_calls_keeps_each_call_with_the_prompt_its_exchanges_were_sent() {
    let scratch = Scratch::new("move-scored-calls");
    let places = scratch.places();
    let path = store_before_scored_calls(&places.lists.join("bae"), true);
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let calls: Vec<(i64, String, Option<i64>, i64)> = conn
        .prepare("SELECT c.id, t.text, c.cost_microdollars, c.tokens_in FROM review_call c JOIN review_text t ON t.hash = c.prompt ORDER BY c.id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(calls.len(), 2);
    let (id, prompt, cost, tokens) = &calls[0];
    assert_eq!((*id, *cost, *tokens), (20, None, 100));
    assert!(
        prompt.starts_with("You review one change.\n\n\n---\n\nGroup: words\n"),
        "the project's instructions are not part of the prompt: {prompt}"
    );
    assert!(
        prompt.contains("Rule text.") && prompt.contains("passes: 2\ncommit messages read: yes")
    );
    assert!(
        calls[1].1.contains("Group: audit r")
            && calls[1].1.contains("passes: 1\ncommit messages read: no")
    );
    let kept: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_exchange e JOIN review_call c ON c.id = e.call_id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 5, "every exchange keeps its call");
    let verdicts: i64 = conn
        .query_row("SELECT COUNT(*) FROM review_verdict", [], |row| row.get(0))
        .unwrap();
    assert_eq!(verdicts, 1);
}

#[test]
fn a_call_with_no_exchange_to_read_its_prompt_from_leaves_the_store_as_it_was() {
    let scratch = Scratch::new("move-scored-calls-bare");
    let places = scratch.places();
    let path = store_before_scored_calls(&places.lists.join("bae"), false);
    let refused = migrate_stores(&places).unwrap_err().text();
    assert!(
        refused.contains("review call 21 has no exchange"),
        "{refused}"
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        db::stamp_of(&conn, &path).unwrap(),
        BEFORE_SCORED_CALLS_STAMP
    );
    let columns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('review_call') WHERE name = 'prompt'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 0);
}

#[test]
fn the_scored_calls_stamp_is_the_schema_before_scored_calls() {
    assert_eq!(
        db::stamp_of_text(&schema_before_scored_calls()),
        BEFORE_SCORED_CALLS_STAMP
    );
}

/// <schema> as the text read before questions and items took tags: without
/// the tags' part.
fn without_tags(schema: String) -> String {
    let schema = without_origins(schema);
    let part = format!("{}\n", db::TAG_PART);
    assert_eq!(
        schema.matches(&part).count(),
        1,
        "the schema has the tags' part once"
    );
    schema.replacen(&part, "", 1)
}

/// The schema text before questions and items took tags.
fn schema_before_tags() -> String {
    without_tags(schema_with_cargo_runs())
}

#[test]
fn the_tags_stamp_is_the_schema_before_tags() {
    assert_eq!(db::stamp_of_text(&schema_before_tags()), BEFORE_TAGS_STAMP);
}

#[test]
fn a_store_from_before_tags_gains_their_table_keeps_its_records_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-tags");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_tags()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_TAGS_STAMP)
            .unwrap();
        plant_run(&conn);
        crate::lists::record::plant(
            &conn,
            "record",
            &[
                ("id", 2_i64.into()),
                ("kind", "question".to_string().into()),
                ("opened_at", 5_i64.into()),
                ("opened_by", "coordinator".to_string().into()),
            ],
        )
        .unwrap();
    }
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let records: i64 = conn
        .query_row("SELECT COUNT(*) FROM record", [], |row| row.get(0))
        .unwrap();
    assert_eq!(records, 1, "the record is kept");
    let tags: i64 = conn
        .query_row("SELECT COUNT(*) FROM record_tag", [], |row| row.get(0))
        .unwrap();
    assert_eq!(tags, 0);
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the old run is kept");
}

/// cargo_runs.sql as it was when builds dropped the table. From the day builds
/// added the table until it dropped it, this text was the schema's last part.
const CARGO_RUNS_PART: &str = r#"-- The record of `builds cargo` runs, in `builds.sqlite` in a project's store
-- folder. cargo_runs.rs owns this table and is the only code that writes it.
-- A row is a fact a run observed: which host ran the build, when it started,
-- and how it ended. The dashboard's host rows read it back.

-- One row per `builds cargo` run, written when the run starts and finished
-- when it ends. A run that dies without ending (a killed `builds`) keeps no
-- end; the dashboard counts a run as running only while its `pid` is alive.
CREATE TABLE IF NOT EXISTS cargo_runs (
    id INTEGER PRIMARY KEY,
    -- When the run started, seconds since 1970.
    started_at INTEGER NOT NULL CHECK (started_at > 0),
    -- When it ended; NULL while it runs.
    ended_at INTEGER CHECK (ended_at >= started_at),
    repo TEXT NOT NULL CHECK (repo <> ''),
    -- The checkout (main, side-N) whose files the run built.
    checkout TEXT NOT NULL CHECK (checkout <> ''),
    -- The host that ran cargo: `local` for this Mac, else a host's name.
    host TEXT NOT NULL CHECK (host <> ''),
    -- The `builds cargo` process on this Mac that started the run.
    pid INTEGER NOT NULL CHECK (pid > 0),
    -- cargo's subcommand and the arguments the caller gave.
    command TEXT NOT NULL CHECK (command <> ''),
    -- cargo's exit status; NULL while it runs.
    exit_code INTEGER,
    CHECK ((ended_at IS NULL) = (exit_code IS NULL))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_cargo_runs_started ON cargo_runs (started_at);
"#;

/// The schema text before builds dropped the cargo runs' table: today's, with
/// that table's part last.
fn schema_with_cargo_runs() -> String {
    format!("{}\n{CARGO_RUNS_PART}", db::schema())
}

#[test]
fn the_cargo_runs_removed_stamp_is_the_schema_with_cargo_runs() {
    assert_eq!(
        db::stamp_of_text(&without_origins(schema_with_cargo_runs())),
        BEFORE_CARGO_RUNS_REMOVED_STAMP
    );
}

#[test]
fn a_store_with_cargo_runs_loses_the_table_and_its_rows_keeps_the_rest_and_equals_a_fresh_schema() {
    let scratch = Scratch::new("move-cargo-runs-removed");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&without_origins(schema_with_cargo_runs()))
            .unwrap();
        conn.pragma_update(None, "user_version", BEFORE_CARGO_RUNS_REMOVED_STAMP)
            .unwrap();
        plant_run(&conn);
        conn.execute(
            "INSERT INTO cargo_runs (started_at, repo, checkout, host, pid, command) VALUES (5, 'bae', 'side-1', 'local', 7, 'test')",
            [],
        )
        .unwrap();
    }
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the move keeps the old run");
}

/// <schema> as its text read before questions named an origin and an origin
/// could be a record of any kind: `item` with an origin that is a decision or
/// a design change and an `origin_kind` that names which, and `question` with
/// no origin.
fn without_origins(schema: String) -> String {
    let mut old = without_finished(schema);
    for (now, then) in [
        (
            "    -- The record it came from, of any kind (trail.rs).
    origin INTEGER,
    CONSTRAINT item_origin_not_itself CHECK (origin <> id),
    CONSTRAINT item_is_a_record FOREIGN KEY (id, record_kind) REFERENCES record (id, kind),
    CONSTRAINT item_origin_is_a_record FOREIGN KEY (origin) REFERENCES record (id)
",
            "    -- The decision or design change that made it.
    origin INTEGER,
    origin_kind TEXT CONSTRAINT item_origin_kind_known CHECK (origin_kind IN ('decision', 'design')),
    CONSTRAINT item_origin_is_whole CHECK ((origin IS NULL) = (origin_kind IS NULL)),
    CONSTRAINT item_is_a_record FOREIGN KEY (id, record_kind) REFERENCES record (id, kind),
    CONSTRAINT item_origin_is_a_record FOREIGN KEY (origin, origin_kind) REFERENCES record (id, kind)
",
        ),
        (
            "    CONSTRAINT question_is_a_record FOREIGN KEY (id, record_kind) REFERENCES record (id, kind),
    CONSTRAINT question_origin_is_a_record FOREIGN KEY (origin) REFERENCES record (id)
) STRICT;

CREATE INDEX question_origin ON question (origin);
",
            "    CONSTRAINT question_is_a_record FOREIGN KEY (id, record_kind) REFERENCES record (id, kind)
) STRICT;
",
        ),
        (
            "    -- The record it came from, of any kind (trail.rs).
    origin INTEGER,
    CONSTRAINT question_origin_not_itself CHECK (origin <> id),
",
            "",
        ),
    ] {
        assert_eq!(old.matches(now).count(), 1, "{now}");
        old = old.replace(now, then);
    }
    old
}

#[test]
fn the_origins_stamp_is_the_schema_before_origins() {
    assert_eq!(
        db::stamp_of_text(&without_origins(db::schema())),
        BEFORE_ORIGINS_STAMP
    );
}

#[test]
fn a_store_from_before_origins_keeps_each_item_its_origin_and_equals_a_fresh_schema() {
    use crate::lists::fixtures::v;
    let scratch = Scratch::new("move-origins");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&without_origins(db::schema())).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_ORIGINS_STAMP)
            .unwrap();
        crate::lists::area::plant(&conn, &[("name", v("Alpha")), ("position", v(0))]).unwrap();
        for (id, kind) in [(1, "decision"), (2, "item"), (3, "question")] {
            crate::lists::record::plant(
                &conn,
                "record",
                &[
                    ("id", v(id)),
                    ("kind", v(kind)),
                    ("opened_at", v(5)),
                    ("opened_by", v("coordinator")),
                ],
            )
            .unwrap();
        }
        crate::lists::item::plant(
            &conn,
            "item",
            &[
                ("id", v(2)),
                ("title", v("Do the work the decision made")),
                ("area", v("Alpha")),
                ("details", v("d")),
                ("audience", v("coordinator")),
                ("origin", v(1)),
                ("origin_kind", v("decision")),
            ],
        )
        .unwrap();
        crate::lists::item::plant(
            &conn,
            "item_priority",
            &[("item_id", v(2)), ("level", v("first")), ("why", v("w"))],
        )
        .unwrap();
        crate::lists::question::plant(
            &conn,
            "question",
            &[
                ("id", v(3)),
                ("title", v("Which way?")),
                ("details", v("d")),
                ("kind", v("question")),
                ("blocks_nothing", v("asked to know")),
            ],
        )
        .unwrap();
        crate::lists::item::plant(
            &conn,
            "item_wait_question",
            &[("item_id", v(2)), ("on_question", v(3))],
        )
        .unwrap();
        plant_run(&conn);
    }
    migrate_stores(&places).unwrap();
    let conn = db::open(&path, db::Make::Never).expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let origins: Vec<(i64, Option<i64>)> = conn
        .prepare(
            "SELECT id, origin FROM item UNION ALL SELECT id, origin FROM question ORDER BY id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(origins, [(2, Some(1)), (3, None)]);
    // The tables that point at `item` and `question` still name them.
    let priorities: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM item_priority JOIN item ON item.id = item_id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(priorities, 1);
    let broken: Option<String> = conn
        .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
        .optional()
        .unwrap();
    assert_eq!(broken, None);
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the move keeps the old run");
}

/// <schema> as its text read before an item could close as finished.
fn without_finished(schema: String) -> String {
    let mut old = without_changed_lines(schema);
    for (now, then) in [
        (
            "'landed', 'finished', 'dropped', 'deferred', 'moved-to-stack')\n    ),",
            "'landed', 'dropped', 'deferred', 'moved-to-stack')\n    ),",
        ),
        (
            "WHEN 'item' THEN how IN ('landed', 'finished', 'dropped', 'deferred', 'moved-to-stack')",
            "WHEN 'item' THEN how IN ('landed', 'dropped', 'deferred', 'moved-to-stack')",
        ),
        (
            "    -- Why whoever closed the record settled, dropped or deferred it, or what a\n    -- finished item made.\n",
            "",
        ),
        (
            "CHECK ((how IN ('settled', 'finished', 'dropped', 'deferred')) = (reason IS NOT NULL))",
            "CHECK ((how IN ('settled', 'dropped', 'deferred')) = (reason IS NOT NULL))",
        ),
    ] {
        assert_eq!(old.matches(now).count(), 1, "{now}");
        old = old.replace(now, then);
    }
    old
}

#[test]
fn the_finished_stamp_is_the_schema_before_finished() {
    assert_eq!(
        db::stamp_of_text(&without_finished(db::schema())),
        BEFORE_FINISHED_STAMP
    );
}

/// A store from before an item could close as finished keeps every closing,
/// equals a fresh schema, and then takes a finished item.
#[test]
fn a_store_from_before_finished_keeps_its_closings_and_takes_a_finished_item() {
    use crate::lists::fixtures::v;
    let scratch = Scratch::new("move-finished");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&without_finished(db::schema())).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_FINISHED_STAMP)
            .unwrap();
        crate::lists::area::plant(&conn, &[("name", v("Alpha")), ("position", v(0))]).unwrap();
        for id in [1, 2] {
            crate::lists::record::plant(
                &conn,
                "record",
                &[
                    ("id", v(id)),
                    ("kind", v("item")),
                    ("opened_at", v(5)),
                    ("opened_by", v("coordinator")),
                ],
            )
            .unwrap();
            crate::lists::item::plant(
                &conn,
                "item",
                &[
                    ("id", v(id)),
                    ("title", v("Audit the screens")),
                    ("area", v("Alpha")),
                    ("details", v("d")),
                    ("audience", v("coordinator")),
                ],
            )
            .unwrap();
        }
        crate::lists::record::plant(
            &conn,
            "closing",
            &[
                ("record_id", v(1)),
                ("kind", v("item")),
                ("how", v("dropped")),
                ("at", v(6)),
                ("by", v("coordinator")),
                ("reason", v("not needed")),
            ],
        )
        .unwrap();
    }
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    drop(conn);
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    let closings = crate::lists::record::read_closings(&conn).unwrap();
    assert_eq!(
        closings[&crate::lists::Id(1)].0,
        crate::lists::How::Dropped("not needed".to_string())
    );
    crate::lists::record::close(
        &conn,
        crate::lists::Id(2),
        crate::lists::How::Finished("items 3 and 4".to_string()),
        &crate::lists::Stamp {
            at: 7,
            by: "coordinator".to_string(),
        },
    )
    .unwrap();
}

/// <schema> as its text read before a review's patch ids left out context
/// lines: `review_commit`'s comment as it read then. The function returns a
/// schema without the review's tables unchanged.
fn without_changed_lines(schema: String) -> String {
    if !schema.contains("CREATE TABLE review_commit") {
        return schema;
    }
    let now = "-- The commits a review read, in order, each with its patch id: `git patch-id
-- --stable` of the commit's diff without context lines, so the id covers only
-- the lines the commit adds and removes (commits.rs). A commit that changes
-- nothing has no patch id, and the review records no row for it. `builds land`
-- matches the commits it lands to a run by these patch ids, so a rebase or a
-- cherry-pick that keeps each commit's added and removed lines keeps its
-- review.
";
    let then = "-- The commits a review read, in order, each with its patch id (`git patch-id
-- --stable`); a commit that changes nothing has none and is left out. `builds
-- land` matches the commits it lands to a run by these patch ids, so a rebase
-- or a cherry-pick that keeps each commit's change keeps its review.
";
    assert_eq!(schema.matches(now).count(), 1, "{now}");
    schema.replace(now, then)
}

#[test]
fn the_changed_lines_stamp_is_the_schema_before_changed_lines() {
    assert_eq!(
        db::stamp_of_text(&without_changed_lines(db::schema())),
        BEFORE_CHANGED_LINES_STAMP
    );
}

/// Makes a store in <dir> with the schema from before a review's patch ids left
/// out context lines. The store holds one landing of repo `demo`: its review
/// read <read>, <fix> fixes its one confirmed finding, and the store keeps a
/// stale patch id for both commits.
fn store_before_changed_lines(dir: &std::path::Path, read: &str, fix: &str) -> std::path::PathBuf {
    use crate::lists::fixtures::v;
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_changed_lines(db::schema()))
        .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_CHANGED_LINES_STAMP)
        .unwrap();
    crate::lists::project::plant(&conn, "repo", &[("name", v("demo"))]).unwrap();
    let plant = |table: &str, columns: &[(&str, rusqlite::types::Value)]| {
        crate::review::record::plant(&conn, table, columns).unwrap();
    };
    plant(
        "review_landing",
        &[
            ("id", v(1)),
            ("key", v("branch:work")),
            ("repo", v("demo")),
            ("opened_at", v(5)),
        ],
    );
    plant(
        "review_run",
        &[
            ("id", v(10)),
            ("landing_id", v(1)),
            ("kind", v("review")),
            ("at", v(6)),
            ("base", v("b")),
            ("head", v("h")),
            ("grouping", v("g")),
            ("rules_version", v("")),
        ],
    );
    plant(
        "review_commit",
        &[
            ("run_id", v(10)),
            ("position", v(0)),
            ("sha", v(read)),
            ("patch_id", v("stale")),
        ],
    );
    plant(
        "review_text",
        &[("hash", v("p")), ("text", v("the prompt"))],
    );
    plant(
        "review_call",
        &[
            ("id", v(20)),
            ("run_id", v(10)),
            ("group_name", v("words")),
            ("model", v("opus")),
            ("prompt", v("p")),
            ("rules", v("[\"r\"]")),
            ("tokens_in", v(1)),
            ("tokens_out", v(1)),
            ("tokens_cached", v(0)),
            ("tokens_written", v(0)),
        ],
    );
    plant(
        "review_finding",
        &[
            ("id", v(30)),
            ("call_id", v(20)),
            ("rule", v("r")),
            ("clause", v("c")),
            ("file", v("f.txt")),
            ("line", v(1)),
            ("code", v("x")),
            ("problem", v("p")),
            ("blocking", v(0)),
            ("settled", v("confirmed")),
            ("settled_why", v("w")),
            ("settled_at", v(7)),
            ("settled_by", v("coordinator")),
        ],
    );
    plant(
        "review_fix",
        &[
            ("finding_id", v(30)),
            ("sha", v(fix)),
            ("patch_id", v("stale")),
            ("at", v(8)),
            ("by", v("coordinator")),
        ],
    );
    path
}

/// The clone `~/dev/demo` under <places>' home with two commits past its
/// start: the one a review read and the one that fixed its finding.
fn demo_clone(places: &Places) -> (String, String) {
    let dir = places.home.join("dev/demo");
    crate::testing::repo(&dir, "target 1G\n");
    let mut shas = Vec::new();
    for (file, text) in [("f.txt", "read\n"), ("g.txt", "fix\n")] {
        std::fs::write(dir.join(file), text).unwrap();
        crate::testing::git(&dir, &["add", file]);
        crate::testing::git(&dir, &["commit", "-q", "-m", file]);
        shas.push(
            crate::review::commits::in_range(&dir, "HEAD~1", "HEAD")
                .unwrap()
                .remove(0),
        );
    }
    (shas[0].clone(), shas[1].clone())
}

#[test]
fn a_store_from_before_changed_lines_keeps_each_review_commit_and_fix_with_its_patch_id_read_again()
{
    let scratch = Scratch::new("move-changed-lines");
    let places = scratch.places();
    let (read, fix) = demo_clone(&places);
    let path = store_before_changed_lines(&places.lists.join("demo"), &read, &fix);
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let today = crate::review::commits::with_patch_ids(
        &places.home.join("dev/demo"),
        &[read.clone(), fix.clone()],
    )
    .unwrap();
    assert_eq!(
        crate::review::record::commits_of_run(&conn, 10).unwrap(),
        today[..1]
    );
    let finding = crate::review::record::finding_of(&conn, 30).unwrap();
    assert_eq!(finding.fix.as_ref(), Some(&today[1]));
}

#[test]
fn a_store_whose_reviewed_commit_the_clone_lacks_is_left_as_it_was() {
    let scratch = Scratch::new("move-changed-lines-lost");
    let places = scratch.places();
    let (_, fix) = demo_clone(&places);
    let lost = "0".repeat(40);
    let path = store_before_changed_lines(&places.lists.join("demo"), &lost, &fix);
    let refused = migrate_stores(&places).unwrap_err().text();
    assert!(
        refused.contains("the move leaves it as it was") && refused.contains("dev/demo"),
        "{refused}"
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        db::stamp_of(&conn, &path).unwrap(),
        BEFORE_CHANGED_LINES_STAMP
    );
    let kept: String = conn
        .query_row("SELECT patch_id FROM review_fix", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, "stale");
}
