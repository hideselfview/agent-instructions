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

/// A store as the schema before `hook_runs.base` made it: that schema's text, its stamp, one run.
fn store_before_hook_base(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(DATABASE);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        &db::schema().replace("\n, base TEXT CHECK (base <> '')) STRICT;", "\n) STRICT;"),
    )
    .unwrap();
    conn.pragma_update(None, "user_version", BEFORE_HOOK_BASE_STAMP)
        .unwrap();
    plant_run(&conn);
    path
}

#[test]
fn a_store_of_the_schema_before_the_base_column_gains_it_keeps_its_runs_and_equals_a_fresh_schema()
{
    let scratch = Scratch::new("move-base");
    let places = scratch.places();
    let path = store_before_hook_base(&places.lists.join("bae"));
    migrate_hook_base(&places).unwrap();
    let conn = db::open_reader(&path)
        .unwrap()
        .expect("opens under today's stamp");
    assert_eq!(
        db::schema_rows(&conn).unwrap(),
        db::schema_rows(&db::memory()).unwrap()
    );
    let stats = crate::hook_stats::stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(
        (stats.runs, stats.recent[0].base.clone()),
        (1, None),
        "the old run is kept, with no base"
    );
    // A run written now records its base, and a second install changes nothing.
    let before = db::schema_rows(&conn).unwrap();
    migrate_hook_base(&places).unwrap();
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
fn a_store_with_a_stamp_the_base_move_does_not_know_is_refused_and_left_as_it_was() {
    let scratch = Scratch::new("move-base-unknown");
    let places = scratch.places();
    let path = store_before_hook_base(&places.lists.join("bae"));
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 1234)
        .unwrap();
    let refused = migrate_hook_base(&places).unwrap_err().text();
    assert!(
        refused.contains("stamp 1234") && refused.contains("does not know"),
        "{refused}"
    );
    let conn = Connection::open(&path).unwrap();
    let columns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('hook_runs') WHERE name = 'base'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(columns, 0);
}

#[test]
fn a_base_move_that_would_not_equal_a_fresh_schema_rolls_back() {
    let scratch = Scratch::new("move-base-drift");
    let places = scratch.places();
    let path = store_before_hook_base(&places.lists.join("bae"));
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP INDEX idx_hook_runs_started")
        .unwrap();
    let refused = migrate_hook_base(&places).unwrap_err().text();
    assert!(
        refused.contains("not the one this builds makes"),
        "{refused}"
    );
    let conn = Connection::open(&path).unwrap();
    let columns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('hook_runs') WHERE name = 'base'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        (columns, db::stamp_of(&conn, &path).unwrap()),
        (0, BEFORE_HOOK_BASE_STAMP)
    );
}

#[test]
fn the_previous_stamp_is_the_schema_without_the_base_column() {
    let text = db::schema().replace("\n, base TEXT CHECK (base <> '')) STRICT;", "\n) STRICT;");
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    assert_eq!(
        i64::from(hash % (i32::MAX as u32 - 2)) + 2,
        BEFORE_HOOK_BASE_STAMP
    );
}
