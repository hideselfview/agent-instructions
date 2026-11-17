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
    let schema = without_design_fix(without_canvas(db::schema_before_app_timing()));
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
    conn.execute_batch(&without_design_fix(without_canvas(
        db::schema_before_app_timing(),
    )))
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
        db::stamp_of_text(&without_design_fix(without_canvas(
            db::schema_before_app_timing()
        ))),
        BEFORE_APP_TIMING_STAMP
    );
}

/// A store as the schema before the design sessions' table made it: that
/// schema's text, its stamp, one run.
fn store_before_design_session(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(without_canvas(
        db::schema_before_design_session(),
    )))
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
        db::stamp_of_text(&without_design_fix(without_canvas(
            db::schema_before_design_session()
        ))),
        BEFORE_DESIGN_SESSION_STAMP
    );
}

/// A store as the schema before the shot findings' table made it: that
/// schema's text, its stamp, one run.
fn store_before_shot_findings(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(without_canvas(
        db::schema_before_shot_findings(),
    )))
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
        db::stamp_of_text(&without_design_fix(without_canvas(
            db::schema_before_shot_findings()
        ))),
        BEFORE_SHOT_FINDINGS_STAMP
    );
}

/// A store as the schema before the review's tables made it: that schema's
/// text, its stamp, one run.
fn store_before_review(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(without_canvas(
        db::schema_before_review(),
    )))
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
        db::stamp_of_text(&without_design_fix(without_canvas(
            db::schema_before_review()
        ))),
        BEFORE_REVIEW_STAMP
    );
}

/// A store as the schema before the cargo runs' table made it: that schema's
/// text, its stamp, one run.
fn store_before_cargo_runs(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_design_fix(schema_before_check_starts()))
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
        db::stamp_of_text(&without_design_fix(schema_before_check_starts())),
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

/// The schema text before builds dropped the cargo runs' table: the one before
/// builds kept each hook check's starting load and recorded cargo runs, with
/// that table's part last.
fn schema_with_cargo_runs() -> String {
    format!("{}\n{CARGO_RUNS_PART}", schema_before_check_starts())
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
        db::stamp_of_text(&without_origins(schema_before_check_starts())),
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
        conn.execute_batch(&without_origins(schema_before_check_starts()))
            .unwrap();
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
        db::stamp_of_text(&without_finished(schema_before_check_starts())),
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
        conn.execute_batch(&without_finished(schema_before_check_starts()))
            .unwrap();
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
        db::stamp_of_text(&without_changed_lines(schema_before_check_starts())),
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
    conn.execute_batch(&without_changed_lines(schema_before_check_starts()))
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
    conn.execute(
        "INSERT INTO review_commit (run_id, position, sha, patch_id) VALUES (10, 0, ?1, 'stale')",
        [read],
    )
    .unwrap();
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
    conn.execute(
        "INSERT INTO review_fix (finding_id, sha, patch_id, at, by) VALUES (30, ?1, 'stale', 8, 'coordinator')",
        [fix],
    )
    .unwrap();
    path
}

/// A store from before a review's patch ids left out context lines moves
/// without reading its commits again: the move drops the commits each run read
/// and the fixes, and the last review closes the finding that named a fix.
#[test]
fn a_store_from_before_changed_lines_moves_and_its_fixed_finding_is_closed() {
    let scratch = Scratch::new("move-changed-lines");
    let places = scratch.places();
    let path = store_before_changed_lines(&places.lists.join("demo"), "read", "fix");
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let finding = crate::review::record::finding_of(&conn, 30).unwrap();
    assert_eq!(finding.closed_by, Some(10));
}

/// <schema> as its text read before builds kept this Mac's load when each
/// hook check started and recorded each `builds cargo` run: without those two
/// parts, the last of the schema.
fn without_check_starts(schema: String) -> String {
    let parts = format!("\n{}\n{}", db::HOOK_CHECK_STARTS_PART, db::CARGO_RUNS_PART);
    assert_eq!(
        schema.matches(&parts).count(),
        1,
        "the schema has the two parts once"
    );
    schema.replacen(&parts, "", 1)
}

/// The schema text before builds kept each hook check's starting load and
/// recorded cargo runs. Each test helper for an older schema starts from this
/// text.
fn schema_before_check_starts() -> String {
    without_check_starts(schema_before_canvas())
}

/// <schema> as its text read before a design change could close as dropped:
/// record.sql's `closing` letting a design change only land, and every newer
/// change to the lists' tables.
fn without_design_drop(schema: String) -> String {
    let schema = without_ranks(schema);
    let (now, then) = (
        "WHEN 'design' THEN how IN ('landed', 'dropped')",
        "WHEN 'design' THEN how = 'landed'",
    );
    assert_eq!(schema.matches(now).count(), 1, "{now}");
    schema.replace(now, then)
}

/// The schema text before a design change could close as dropped.
fn schema_before_design_drop() -> String {
    without_design_drop(schema_before_pushed())
}

#[test]
fn the_design_drop_stamp_is_the_schema_before_design_drop() {
    assert_eq!(
        db::stamp_of_text(&schema_before_design_drop()),
        BEFORE_DESIGN_DROP_STAMP
    );
}

/// A store from before a design change could close as dropped keeps its
/// closings, equals a fresh schema, and then takes a dropped design change.
#[test]
fn a_store_from_before_design_drop_keeps_its_closings_and_takes_a_dropped_change() {
    use crate::lists::fixtures::v;
    let scratch = Scratch::new("move-design-drop");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_design_drop()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_DESIGN_DROP_STAMP)
            .unwrap();
        crate::lists::project::plant(
            &conn,
            "project",
            &[("singleton", v(1)), ("name", v("bae")), ("canvas", v(1))],
        )
        .unwrap();
        crate::lists::project::plant(&conn, "repo", &[("name", v("bae"))]).unwrap();
        for id in [1, 2] {
            crate::lists::record::plant(
                &conn,
                "record",
                &[
                    ("id", v(id)),
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
                    ("id", v(id)),
                    ("title", v("A design change")),
                    ("details", v("d")),
                    ("repo", v("bae")),
                    ("worktree", v("/d/wt")),
                ],
            )
            .unwrap();
        }
        crate::lists::record::plant(
            &conn,
            "closing",
            &[
                ("record_id", v(1)),
                ("kind", v("design")),
                ("how", v("landed")),
                ("at", v(6)),
                ("by", v("tester")),
            ],
        )
        .unwrap();
        crate::lists::record::plant(
            &conn,
            "closing_commit",
            &[
                ("record_id", v(1)),
                ("how", v("landed")),
                ("position", v(0)),
                ("sha", v("a".repeat(40))),
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
        crate::lists::How::Landed(vec!["a".repeat(40)])
    );
    crate::lists::record::close(
        &conn,
        crate::lists::Id(2),
        crate::lists::How::Dropped("another change covers it".to_string()),
        &crate::lists::Stamp {
            at: 7,
            by: "tester".to_string(),
        },
    )
    .unwrap();
}

/// <schema> as its text read before a project said whether it publishes its
/// boards as a canvas: project.sql without `canvas`, and every newer change.
fn without_canvas(schema: String) -> String {
    let mut old = without_design_drop(schema);
    for (now, then) in [
        (
            "-- The project: its name, whether it publishes its boards as a canvas, the
-- repos its lists cover, and each repo's globs for design places,
-- design-system sources and spec files. project.rs owns these tables and is
-- the only code that writes them.
",
            "-- The project: its name, the repos its lists cover, and each repo's globs for
-- design places, design-system sources and spec files. project.rs owns these
-- tables and is the only code that writes them.
",
        ),
        (
            "    ),
    -- 1 when the project publishes its boards as a claude.ai canvas: each of
    -- its design changes gives every board's canvas link, and landing one
    -- ends with publishing the canvas. 0 when its boards stay in the repo.
    canvas INTEGER NOT NULL CONSTRAINT project_canvas_is_yes_or_no CHECK (canvas IN (0, 1))
) STRICT;",
            "    )
) STRICT;",
        ),
    ] {
        assert_eq!(old.matches(now).count(), 1, "{now}");
        old = old.replace(now, then);
    }
    old
}

/// The schema text before a project said whether it publishes a canvas.
fn schema_before_canvas() -> String {
    without_canvas(schema_before_pushed())
}

#[test]
fn the_canvas_stamp_is_the_schema_before_canvas() {
    assert_eq!(
        db::stamp_of_text(&schema_before_canvas()),
        BEFORE_CANVAS_STAMP
    );
}

/// Makes a store of project <name> in <places> with the schema from before
/// a project said whether it publishes a canvas, holding one design change
/// with a canvas link when <linked>.
fn store_before_canvas(places: &Places, name: &str, linked: bool) -> std::path::PathBuf {
    let dir = places.lists.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&schema_before_canvas()).unwrap();
    conn.pragma_update(None, "user_version", BEFORE_CANVAS_STAMP)
        .unwrap();
    let text = |t: &str| rusqlite::types::Value::Text(t.to_string());
    crate::lists::project::plant(
        &conn,
        "project",
        &[("singleton", 1.into()), ("name", text(name))],
    )
    .unwrap();
    if linked {
        crate::lists::project::plant(&conn, "repo", &[("name", text(name))]).unwrap();
        crate::lists::record::plant(
            &conn,
            "record",
            &[
                ("id", 1.into()),
                ("kind", text("design")),
                ("opened_at", 1.into()),
                ("opened_by", text("tester")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design",
            &[
                ("id", 1.into()),
                ("title", text("A design change")),
                ("details", text("d")),
                ("repo", text(name)),
                ("worktree", text("/d/wt")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design_place",
            &[
                ("design_id", 1.into()),
                ("path", text("design/mocks/A.dc.html")),
                ("matches", text("the app draws it")),
            ],
        )
        .unwrap();
        crate::lists::design::plant(
            &conn,
            "design_board",
            &[
                ("design_id", 1.into()),
                ("path", text("design/mocks/A.dc.html")),
                ("link", text("https://canvas.example/#A")),
            ],
        )
        .unwrap();
    }
    path
}

/// A store from before a project said whether it publishes a canvas gets
/// `canvas`: set when one of its design changes holds a canvas link, clear
/// otherwise, and the store equals a fresh schema.
#[test]
fn a_store_from_before_canvas_publishes_one_only_when_its_changes_hold_a_link() {
    let scratch = Scratch::new("move-canvas");
    let places = scratch.places();
    let linked = store_before_canvas(&places, "bae", true);
    let unlinked = store_before_canvas(&places, "builds", false);
    migrate_stores(&places).unwrap();
    for (path, canvas) in [(linked, true), (unlinked, false)] {
        let conn = db::open_reader(&path)
            .unwrap()
            .expect("opens under today's stamp");
        assert_eq!(
            db::schema_rows(&conn).unwrap(),
            db::schema_rows(&db::memory()).unwrap()
        );
        assert_eq!(
            crate::lists::project::read(&conn).unwrap().canvas,
            canvas,
            "{}",
            path.display()
        );
    }
}

#[test]
fn the_check_starts_stamp_is_the_schema_before_check_starts() {
    assert_eq!(
        db::stamp_of_text(&schema_before_check_starts()),
        BEFORE_CHECK_STARTS_STAMP
    );
}

/// A store from before the `hook_check_starts` and `builds_cargo_runs` tables
/// keeps its runs, gains the two empty tables and equals a fresh schema.
#[test]
fn a_store_from_before_check_starts_keeps_its_runs_and_gains_the_tables() {
    let scratch = Scratch::new("move-check-places");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_check_starts()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_CHECK_STARTS_STAMP)
            .unwrap();
        plant_run(&conn);
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

/// <schema> without each of <parts>, which it holds once each.
fn without_parts(mut schema: String, parts: &[&str]) -> String {
    for part in parts {
        let part = format!("\n{part}");
        assert_eq!(
            schema.matches(&part).count(),
            1,
            "the schema has the part once"
        );
        schema = schema.replacen(&part, "", 1);
    }
    schema
}

/// The schema text before a review closed the confirmed findings its changes
/// fix: the tables of fixes and of read and pushed commits, and of findings
/// whose code left their branch, in place of `review_closed`, and the review's
/// comments as they read then.
fn schema_before_closed() -> String {
    let mut schema = db::schema();
    for (now, then) in [
        (
            "-- Every review of a change against the rules (`builds review`): the landing it
-- belongs to, each run with the net change it read, each reviewer call, its verdict
-- for every rule, each finding with how the coordinator settled it, and the
-- run that closed a confirmed finding. record.rs owns these tables and is the
-- only code that writes them.",
            "-- Every review of a change against the rules (`builds review`): the landing it
-- belongs to, each run with the net change it read, each reviewer call, its verdict
-- for every rule, each finding with how the coordinator settled it, the
-- commit that fixed a confirmed finding, and each confirmed finding whose
-- code is no longer in its branch's change. record.rs owns these tables and is
-- the only code that writes them.",
        ),
        (
            "CREATE INDEX review_finding_call ON review_finding (call_id);

-- A second settle: the coordinator changed an open confirmed finding to
-- dropped. review_finding keeps the first settle, and this row keeps the
-- second, which is how the finding counts from then on.
",
            "CREATE INDEX review_finding_call ON review_finding (call_id);

-- The commits of the net change a review run found, in order, each with its
-- patch id: `git patch-id --stable` of the commit's diff without context
-- lines, so the id covers only the lines the commit adds and removes
-- (commits.rs). A commit that changes nothing has no patch id, and the review
-- records no row for it. `builds land` finds the review that a landing's
-- commits belong to by these patch ids, and `review replay` finds a landed
-- change's commits by them once its branch is gone.
CREATE TABLE review_commit (
    run_id INTEGER NOT NULL REFERENCES review_run (id),
    position INTEGER NOT NULL CONSTRAINT review_commit_position_not_negative CHECK (position >= 0),
    sha TEXT NOT NULL,
    patch_id TEXT NOT NULL,
    PRIMARY KEY (run_id, position)
) STRICT, WITHOUT ROWID;

-- The commit that fixes a confirmed finding, and its patch id, which must be
-- among the patch ids of the commits the landing pushes.
CREATE TABLE review_fix (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    sha TEXT NOT NULL,
    patch_id TEXT NOT NULL,
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_fix_by_says_something CHECK (trim(by) <> '')
) STRICT;

-- Each commit of a landing that `builds land` pushed, by the patch id of its
-- added and removed lines. A branch lands in parts: until a push leaves its
-- branch nothing to land, its landing stays open and counts as landed only for
-- these commits.
CREATE TABLE review_pushed (
    landing_id INTEGER NOT NULL REFERENCES review_landing (id),
    patch_id TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (landing_id, patch_id)
) STRICT, WITHOUT ROWID;

-- A second settle: the coordinator changed a confirmed finding that names no
-- fix to dropped. review_finding keeps the first settle, and this row keeps
-- the second, which is how the finding counts from then on.
",
        ),
        (
            "-- A confirmed finding that a later review run closed: the changes that run
-- read fix it. builds gives each review the open confirmed findings of the
-- groups it asks, and each reviewer names the ones the changes fix. A
-- confirmed finding that no run closed and the coordinator did not settle
-- again as dropped is open, and `builds land` refuses the branch while one is.
CREATE TABLE review_closed (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    run_id INTEGER NOT NULL REFERENCES review_run (id)
) STRICT;
",
            "-- A confirmed finding that names no fix and whose quoted code is on no line of
-- its branch's change past origin/main, so the finding needs no fix. base and
-- head are the commits of the change that the builds tool read.
CREATE TABLE review_code_left (
    finding_id INTEGER PRIMARY KEY REFERENCES review_finding (id),
    base TEXT NOT NULL,
    head TEXT NOT NULL,
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT review_code_left_by_says_something CHECK (trim(by) <> '')
) STRICT;
",
        ),
    ] {
        assert_eq!(schema.matches(now).count(), 1, "{now}");
        schema = schema.replacen(now, then, 1);
    }
    schema
}

#[test]
fn the_closed_stamp_is_the_schema_before_closed_findings() {
    assert_eq!(
        db::stamp_of_text(&schema_before_closed()),
        BEFORE_CLOSED_STAMP
    );
}

/// A store from before a review closed the findings its changes fix moves
/// with each confirmed finding that named a fix, or whose code left its
/// branch, closed by its landing's last review, and the rest open.
#[test]
fn a_store_from_before_closed_findings_closes_the_fixed_ones_with_the_last_review() {
    let scratch = Scratch::new("move-closed");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_closed()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_CLOSED_STAMP)
            .unwrap();
        crate::lists::project::plant(&conn, "repo", &[("name", "bae".to_string().into())]).unwrap();
        conn.execute_batch(
            "INSERT INTO review_landing (id, key, repo, opened_at) VALUES (1, 'k', 'bae', 5);
            INSERT INTO review_run (id, landing_id, kind, at, base, head, grouping, rules_version) VALUES (7, 1, 'review', 6, 'm', 'h', 'g', '');
            UPDATE review_landing SET reviewed_run = 7;
            INSERT INTO review_text (hash, text) VALUES ('p', 'prompt');
            INSERT INTO review_call (id, run_id, group_name, model, prompt, rules, tokens_in, tokens_out, tokens_cached, tokens_written) VALUES (1, 7, 'alpha', 'sonnet', 'p', '[]', 0, 0, 0, 0);
            INSERT INTO review_finding (id, call_id, rule, clause, file, line, code, problem, blocking, settled, settled_why, settled_at, settled_by)
                VALUES (1, 1, 'r', 'c', 'a.rs', 1, 'x', 'p', 1, 'confirmed', 'real', 6, 'me'),
                    (2, 1, 'r', 'c', 'a.rs', 2, 'y', 'p', 1, 'confirmed', 'real', 6, 'me'),
                    (3, 1, 'r', 'c', 'a.rs', 3, 'z', 'p', 1, 'confirmed', 'real', 6, 'me');
            INSERT INTO review_fix (finding_id, sha, patch_id, at, by) VALUES (1, 's', 'p', 7, 'me');
            INSERT INTO review_code_left (finding_id, base, head, at, by) VALUES (2, 'm', 'h', 7, 'me');
            INSERT INTO review_commit (run_id, position, sha, patch_id) VALUES (7, 0, 'h', 'p');
            INSERT INTO review_pushed (landing_id, patch_id, at) VALUES (1, 'p', 8);",
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
    let closed: Vec<Option<i64>> = crate::review::record::findings_of_landing(&conn, 1)
        .unwrap()
        .iter()
        .map(|f| f.closed_by)
        .collect();
    assert_eq!(closed, [Some(7), Some(7), None]);
}

/// The schema text before each landing named the run of its last review:
/// `review_landing` without `reviewed_run`.
fn schema_before_reviewed_run() -> String {
    let column = "    -- The last review: the run whose net change (base...head) the review has
    -- read. A review shows the changes since the last review, and nothing
    -- else. builds review writes a landing together with its first run, which
    -- it records here in the same write.
    reviewed_run INTEGER REFERENCES review_run (id),
";
    let schema = schema_before_closed();
    assert_eq!(schema.matches(column).count(), 1, "{column}");
    schema.replacen(column, "", 1)
}

#[test]
fn the_reviewed_run_stamp_is_the_schema_before_reviewed_runs() {
    assert_eq!(
        db::stamp_of_text(&schema_before_reviewed_run()),
        BEFORE_REVIEWED_RUN_STAMP
    );
}

/// A store from before each landing named its last review moves with each
/// landing's newest run that read a net change no earlier run of it read: not
/// a run that read a stretch of commits, and not a run that asked the groups
/// that gave an earlier run no answer.
#[test]
fn a_store_from_before_reviewed_runs_names_each_landings_last_review() {
    let scratch = Scratch::new("move-reviewed-run");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_reviewed_run()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_REVIEWED_RUN_STAMP)
            .unwrap();
        crate::lists::project::plant(&conn, "repo", &[("name", "bae".to_string().into())]).unwrap();
        let first = crate::review::record::add_landing(&conn, "k", "bae", 5).unwrap();
        let second = crate::review::record::add_landing(&conn, "k2", "bae", 5).unwrap();
        for (id, landing, base, head, since) in [
            (7, first, "m", "h1", None),
            (8, first, "h1", "h2", None),
            (9, first, "m", "h3", Some(8)),
            (10, first, "m", "h1", None),
            (11, second, "m", "h1", None),
            (12, second, "h1", "h2", None),
        ] {
            conn.execute(
                "INSERT INTO review_run (id, landing_id, kind, at, base, head, since_run, grouping, rules_version) VALUES (?1, ?2, 'review', 6, ?3, ?4, ?5, 'g', '')",
                rusqlite::params![id, landing, base, head, since],
            )
            .unwrap();
        }
    }
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let reviewed = |landing| {
        crate::review::record::landing(&conn, landing)
            .unwrap()
            .reviewed_run
    };
    assert_eq!((reviewed(1), reviewed(2)), (Some(9), Some(11)));
}

/// The schema text before a later review run read only how the branch's net
/// change differs from the net change an earlier run read: `review_run`
/// without `since_run`, and the review's comments as they read then.
fn schema_before_net_change() -> String {
    let mut schema = schema_before_reviewed_run();
    for (now, then) in [
        (
            "-- belongs to, each run with the net change it read, each reviewer call, its verdict",
            "-- belongs to, each run and the commits it read, each reviewer call, its verdict",
        ),
        (
            "-- of a landing that landed. A review run's base and head are the branch's net
-- change as the run found it: the common ancestor with the base it reviews
-- against, and the branch's head.
",
            "-- of a landing that landed.
",
        ),
        (
            "    -- For a review run that read only how the net change from base to head
    -- differs from the net change an earlier run of its review read, that
    -- earlier run; null for a run that read the whole net change.
    since_run INTEGER REFERENCES review_run (id),
",
            "",
        ),
        (
            "    CONSTRAINT review_run_review_has_a_landing CHECK (kind = 'replay' OR landing_id IS NOT NULL),
    CONSTRAINT review_run_since_only_in_a_review CHECK (since_run IS NULL OR kind = 'review')
",
            "    CONSTRAINT review_run_review_has_a_landing CHECK (kind = 'replay' OR landing_id IS NOT NULL)
",
        ),
        (
            "-- The commits of the net change a review run found, in order, each with its
-- patch id: `git patch-id --stable` of the commit's diff without context
-- lines, so the id covers only the lines the commit adds and removes
-- (commits.rs). A commit that changes nothing has no patch id, and the review
-- records no row for it. `builds land` finds the review that a landing's
-- commits belong to by these patch ids, and `review replay` finds a landed
-- change's commits by them once its branch is gone.",
            "-- The commits a review read, in order, each with its patch id: `git patch-id
-- --stable` of the commit's diff without context lines, so the id covers only
-- the lines the commit adds and removes (commits.rs). A commit that changes
-- nothing has no patch id, and the review records no row for it. `builds land`
-- matches the commits it lands to a run by these patch ids, so a rebase or a
-- cherry-pick that keeps each commit's added and removed lines keeps its
-- review.",
        ),
    ] {
        assert_eq!(schema.matches(now).count(), 1, "{now}");
        schema = schema.replacen(now, then, 1);
    }
    schema
}

#[test]
fn the_net_change_stamp_is_the_schema_before_net_changes() {
    assert_eq!(
        db::stamp_of_text(&schema_before_net_change()),
        BEFORE_NET_CHANGE_STAMP
    );
}

/// A store from before a later review run read only the difference between
/// net changes keeps each run, as one that read a whole net change, and
/// equals a fresh schema.
#[test]
fn a_store_from_before_net_changes_keeps_its_runs_each_reading_a_whole_net_change() {
    let scratch = Scratch::new("move-net-change");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_net_change()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_NET_CHANGE_STAMP)
            .unwrap();
        crate::lists::project::plant(&conn, "repo", &[("name", "bae".to_string().into())]).unwrap();
        let landing = crate::review::record::add_landing(&conn, "k", "bae", 5).unwrap();
        conn.execute(
            "INSERT INTO review_run (id, landing_id, kind, at, base, head, grouping, rules_version) VALUES (7, ?1, 'review', 6, 'b', 'h', 'g', '')",
            [landing],
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
    let runs = crate::review::record::runs_of_landing(&conn, 1).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (
            runs[0].id,
            runs[0].base.as_str(),
            runs[0].head.as_str(),
            runs[0].since
        ),
        (7, "b", "h", None)
    );
}

/// The schema text before the coordinator could settle a finding again:
/// without the tables of second settles and of confirmed findings whose code
/// is no longer in their branch's change, and with the review's header as it
/// read then.
fn schema_before_code_left() -> String {
    let schema = schema_before_net_change();
    let start = schema
        .find("\n-- A second settle: the coordinator")
        .expect("the second settles' comment");
    let last = start
        + schema[start..]
            .find("CREATE TABLE review_code_left")
            .expect("the review_code_left table");
    let end = "STRICT;\n";
    let stop = last + schema[last..].find(end).expect("it ends") + end.len();
    let schema = format!("{}{}", &schema[..start], &schema[stop..]);
    let (now, then) = (
        "-- for every rule, each finding with how the coordinator settled it, the
-- commit that fixed a confirmed finding, and each confirmed finding whose
-- code is no longer in its branch's change. record.rs owns these tables and is
-- the only code that writes them.",
        "-- for every rule, each finding with how the coordinator settled it, and the
-- commit that fixed a confirmed finding. record.rs owns these tables and is the
-- only code that writes them.",
    );
    assert_eq!(schema.matches(now).count(), 1, "{now}");
    schema.replacen(now, then, 1)
}

#[test]
fn the_code_left_stamp_is_the_schema_before_code_left() {
    assert_eq!(
        db::stamp_of_text(&schema_before_code_left()),
        BEFORE_CODE_LEFT_STAMP
    );
}

/// A store from before the coordinator could settle a finding again keeps its
/// runs, equals a fresh schema, and then records a second settle.
#[test]
fn a_store_from_before_code_left_moves_and_records_a_second_settle() {
    let scratch = Scratch::new("move-code-left");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_code_left()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_CODE_LEFT_STAMP)
            .unwrap();
        plant_run(&conn);
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
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the move keeps the old run");
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    crate::lists::project::plant(&conn, "repo", &[("name", "bae".to_string().into())]).unwrap();
    let landing = crate::review::record::add_landing(&conn, "k", "bae", 5).unwrap();
    let finding = |code: &str| crate::review::record::NewFinding {
        rule: "r".to_string(),
        clause: "c".to_string(),
        file: "a.rs".to_string(),
        line: 1,
        code: code.to_string(),
        problem: "p".to_string(),
        blocking: true,
        check_failed: None,
    };
    let call = crate::review::record::NewCall {
        group: "g".to_string(),
        model: "sonnet".to_string(),
        prompt: "the prompt".to_string(),
        rules: vec!["r".to_string()],
        tokens_in: 0,
        tokens_out: 0,
        tokens_cached: 0,
        tokens_written: 0,
        cost_microdollars: 0,
        sites: None,
        failure: None,
        verdicts: vec![("r".to_string(), "finding".to_string())],
        findings: vec![finding("wrong"), finding("gone")],
        closes: Vec::new(),
        exchanges: Vec::new(),
    };
    let run = crate::review::record::NewRun {
        landing: Some(landing),
        kind: crate::review::record::Kind::Review,
        label: None,
        at: 6,
        base: "b",
        head: "h",
        since: None,
        grouping: "g",
        rules_version: "",
    };
    let (_, ids) = crate::review::record::add_run(&conn, &run, &[call]).unwrap();
    for id in &ids[0] {
        crate::review::record::settle(&conn, *id, "confirmed", "real", 7, "me").unwrap();
    }
    crate::review::record::settle_again(&conn, ids[0][0], "wrong", 8, "me").unwrap();
}

/// The schema text before builds recorded each check of a brief against the
/// product spec: without the spec checks' tables.
fn schema_before_spec_check() -> String {
    without_parts(schema_before_code_left(), &[db::SPEC_CHECK_PART])
}

/// The schema text before an item could hold a worktree parked or record
/// its dropped work: without the parked and dropped items' tables.
fn schema_before_parked() -> String {
    without_parts(
        schema_before_spec_check(),
        &[db::PARKED_PART, db::DROPPED_PART],
    )
}

#[test]
fn the_spec_check_stamp_is_the_schema_before_spec_checks() {
    assert_eq!(
        db::stamp_of_text(&schema_before_spec_check()),
        BEFORE_SPEC_CHECK_STAMP
    );
}

/// A store from before spec checks keeps its parked item, gains the empty
/// tables, equals a fresh schema, and then records a check of the item's
/// brief.
#[test]
fn a_store_from_before_spec_checks_gains_the_tables_and_records_a_check() {
    use crate::lists::fixtures::v;
    let scratch = Scratch::new("move-spec-check");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_spec_check()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_SPEC_CHECK_STAMP)
            .unwrap();
        crate::lists::project::plant(
            &conn,
            "project",
            &[("singleton", v(1)), ("name", v("bae")), ("canvas", v(1))],
        )
        .unwrap();
        crate::lists::project::plant(&conn, "repo", &[("name", v("bae"))]).unwrap();
        crate::lists::area::plant(&conn, &[("name", v("Alpha")), ("position", v(0))]).unwrap();
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
                ("title", v("Do the first thing now")),
                ("area", v("Alpha")),
                ("details", v("d")),
                ("audience", v("coordinator")),
            ],
        )
        .unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        crate::lists::parked::park(
            &conn,
            crate::lists::Id(1),
            &[crate::lists::parked::Parked {
                worktree: "/d/bae-worktrees/side-2".to_string(),
                branch: "work".to_string(),
                since: crate::lists::Stamp {
                    at: 7,
                    by: "tester".to_string(),
                },
            }],
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
    let parked: i64 = conn
        .query_row("SELECT COUNT(*) FROM parked", [], |row| row.get(0))
        .unwrap();
    assert_eq!(parked, 1, "the move keeps the parked item");
    crate::lists::spec_check::tests::record_one(&conn, "bae", crate::lists::Id(1));
}

#[test]
fn the_parked_stamp_is_the_schema_before_parked_items() {
    assert_eq!(
        db::stamp_of_text(&schema_before_parked()),
        BEFORE_PARKED_STAMP
    );
}

/// A store from before parked items keeps its items, gains the empty tables,
/// equals a fresh schema, and then takes a parked item.
#[test]
fn a_store_from_before_parked_items_gains_the_tables_and_parks_an_item() {
    use crate::lists::fixtures::v;
    let scratch = Scratch::new("move-parked");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_parked()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_PARKED_STAMP)
            .unwrap();
        crate::lists::project::plant(
            &conn,
            "project",
            &[("singleton", v(1)), ("name", v("bae")), ("canvas", v(1))],
        )
        .unwrap();
        crate::lists::project::plant(&conn, "repo", &[("name", v("bae"))]).unwrap();
        crate::lists::area::plant(&conn, &[("name", v("Alpha")), ("position", v(0))]).unwrap();
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
                ("title", v("Do the first thing now")),
                ("area", v("Alpha")),
                ("details", v("d")),
                ("audience", v("coordinator")),
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
    crate::lists::parked::park(
        &conn,
        crate::lists::Id(1),
        &[crate::lists::parked::Parked {
            worktree: "/d/bae-worktrees/side-2".to_string(),
            branch: "work".to_string(),
            since: crate::lists::Stamp {
                at: 7,
                by: "tester".to_string(),
            },
        }],
    )
    .unwrap();
}

/// The schema text before builds recorded each commit a landing pushed:
/// without the `review_pushed` table, and with `review_landing`'s comment as
/// it read then.
fn schema_before_pushed() -> String {
    let schema = schema_before_parked();
    let table = db::statement(
        &schema,
        "\n-- Each commit of a landing that",
        "WITHOUT ROWID;\n",
    );
    let schema = schema.replacen(table, "", 1);
    let now = "landed_at is when `builds land` pushed the
-- commits that left its branch nothing to land, and landed_run the run that
-- covered them.";
    let then = "landed_at is when `builds land` pushed it, and
-- landed_run the run that covered the commits it pushed.";
    assert_eq!(
        schema.matches(now).count(),
        1,
        "the schema has the comment once"
    );
    schema.replacen(now, then, 1)
}

#[test]
fn the_pushed_stamp_is_the_schema_before_pushed_commits() {
    assert_eq!(
        db::stamp_of_text(&without_ranks(schema_before_pushed())),
        BEFORE_PUSHED_STAMP
    );
}

/// Creates a store in <dir> with the schema from before `review_pushed`: that
/// schema's text, its stamp and one run. Install moves it to today's schema.
pub fn store_before_pushed(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&without_ranks(schema_before_pushed()))
        .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_PUSHED_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

/// A store from before `review_pushed` keeps its runs and equals a fresh
/// schema.
#[test]
fn a_store_from_before_pushed_commits_keeps_its_runs() {
    let scratch = Scratch::new("move-pushed");
    let places = scratch.places();
    let path = store_before_pushed(&places.lists.join("bae"));
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

/// <schema> as its text read before each question placed first had a rank:
/// question.sql's `question_placement` without `rank`.
fn without_ranks(schema: String) -> String {
    let (now, then) = (
        "    at TEXT NOT NULL CONSTRAINT question_placement_at_known CHECK (at IN ('first', 'last')),
    -- A question placed first sits in the person's ranked order at the top
    -- of the stack, above every question with a lower rank. A question
    -- placed last has no rank.
    rank INTEGER CONSTRAINT question_placement_rank_once UNIQUE,
    why TEXT NOT NULL CONSTRAINT question_placement_why_says_something CHECK (trim(why) <> ''),
    by_person INTEGER NOT NULL CONSTRAINT question_placement_by_person_is_a_flag CHECK (by_person IN (0, 1)),
    CONSTRAINT question_placement_rank_only_first CHECK ((at = 'first') = (rank IS NOT NULL))
) STRICT;",
        "    at TEXT NOT NULL CONSTRAINT question_placement_at_known CHECK (at IN ('first', 'last')),
    why TEXT NOT NULL CONSTRAINT question_placement_why_says_something CHECK (trim(why) <> ''),
    by_person INTEGER NOT NULL CONSTRAINT question_placement_by_person_is_a_flag CHECK (by_person IN (0, 1))
) STRICT;",
    );
    assert_eq!(schema.matches(now).count(), 1, "{now}");
    schema.replacen(now, then, 1)
}

/// The schema text before each question placed first had a rank.
fn schema_before_ranks() -> String {
    without_ranks(schema_before_parked())
}

#[test]
fn the_ranks_stamp_is_the_schema_before_ranks() {
    assert_eq!(
        db::stamp_of_text(&schema_before_ranks()),
        BEFORE_RANKS_STAMP
    );
}

/// A store from before ranks gives each question placed first a rank by when
/// it was asked, the newest the highest, so the stack keeps its order, and
/// equals a fresh schema.
#[test]
fn a_store_from_before_ranks_ranks_the_questions_placed_first_newest_asked_on_top() {
    use crate::lists::fixtures::v;
    let scratch = Scratch::new("move-ranks");
    let places = scratch.places();
    let dir = places.lists.join("bae");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DATABASE);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&schema_before_ranks()).unwrap();
        conn.pragma_update(None, "user_version", BEFORE_RANKS_STAMP)
            .unwrap();
        // The test asks each question at the given time and places it as
        // given.
        for (id, asked, at) in [
            (1, 30, Some("first")),
            (2, 10, Some("first")),
            (3, 20, Some("last")),
            (4, 40, Some("first")),
            (5, 50, None),
        ] {
            crate::lists::record::plant(
                &conn,
                "record",
                &[
                    ("id", v(id)),
                    ("kind", v("question")),
                    ("opened_at", v(asked)),
                    ("opened_by", v("tester")),
                ],
            )
            .unwrap();
            crate::lists::question::plant(
                &conn,
                "question",
                &[
                    ("id", v(id)),
                    ("title", v("A question")),
                    ("details", v("d")),
                    ("kind", v("question")),
                    ("blocks_nothing", v("asked anyway")),
                ],
            )
            .unwrap();
            if let Some(at) = at {
                crate::lists::question::plant(
                    &conn,
                    "question_placement",
                    &[
                        ("question_id", v(id)),
                        ("at", v(at)),
                        ("why", v("w")),
                        ("by_person", v(1)),
                    ],
                )
                .unwrap();
            }
        }
    }
    migrate_stores(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let ranks: Vec<(u32, Option<i64>)> = conn
        .prepare("SELECT question_id, rank FROM question_placement ORDER BY question_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        ranks,
        vec![(1, Some(2)), (2, Some(1)), (3, None), (4, Some(3))]
    );
}
