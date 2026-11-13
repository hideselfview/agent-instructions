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
        assert!(std::time::Instant::now() < deadline, "{} stayed held", path.display());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn the_old_files_become_the_new_ones_with_the_lock_keeping_its_inode() {
    let scratch = Scratch::new("migrate");
    let places = old_volume(&scratch);
    let dir = places.root.join("demo");
    let inode = std::fs::metadata(dir.join("app-android.lock")).unwrap().ino();
    migrate_app_files(&places).unwrap();
    assert_eq!(places.app_checkouts_of("demo", Lane::Android).unwrap(), vec!["side-3"]);
    assert!(!dir.join("app-android-checkout").exists() && !dir.join("app-android.lock").exists());
    let lock = places.app_lock_file("demo", Lane::Android, "side-3");
    assert_eq!(std::fs::metadata(lock).unwrap().ino(), inode, "the lock a holder has open is the new lock");
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
    assert_eq!(places.app_checkouts_of("demo", Lane::Android).unwrap(), vec!["side-3"]);
    assert!(dir.join("app-android-checkout").is_file() && dir.join("app-android.lock").is_file(), "the old names stay");
    assert!(crate::queue::try_lock(&new_lock).unwrap().is_none(), "the new path is the lock the old build holds");
    unsafe { libc::close(held) };
    until_free(&dir.join("app-android.lock"));

    // A ticket of an old waiter keeps them too.
    let ticket = crate::queue::open_with(&dir.join("app-android.queue/123-1"), libc::O_CREAT | libc::O_CLOEXEC).unwrap();
    migrate_app_files(&places).unwrap();
    assert!(dir.join("app-android.lock").is_file());
    unsafe { libc::close(ticket) };
    until_free(&dir.join("app-android.queue/123-1"));

    migrate_app_files(&places).unwrap();
    assert!(!dir.join("app-android-checkout").exists() && !dir.join("app-android.lock").exists());
    assert_eq!(places.app_checkouts_of("demo", Lane::Android).unwrap(), vec!["side-3"]);
    let fd = crate::queue::try_lock(&new_lock).unwrap().expect("free once the old build ended");
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
    let ticket = crate::queue::open_with(&dir.join("app.queue/9-1"), libc::O_CREAT | libc::O_CLOEXEC).unwrap();
    migrate_app_files(&places).unwrap();
    assert_eq!(
        std::fs::metadata(dir.join("app.lock")).unwrap().ino(),
        std::fs::metadata(places.app_lock_file("demo", Lane::Mac, "side-1")).unwrap().ino()
    );
    unsafe { libc::close(ticket) };
}

/// The lists of a project holding one of everything: every kind of record, every
/// kind of closing, waits of all four kinds, an item running in two repos,
/// landed parts and notes, and a design change with its places, boards, review
/// page and approval, in an in-memory database.
fn full_lists() -> Connection {
    use crate::lists::decision::{NewDecision, add_decision};
    use crate::lists::design::{Board, Resolution, ReviewBoard, ReviewSection, Theme, save};
    use crate::lists::fixtures::{NOON, db, design, draft, item, question, read, sha, stamp};
    use crate::lists::item::{Note, Priority, add_note, add_wait, set_priority};
    use crate::lists::landing::add_part;
    use crate::lists::question::{NewQuestion, Placement, PlanRef, Position, QuestionKind, ask};
    use crate::lists::record::close;
    use crate::lists::running::start;
    use crate::lists::{Level, LandedPart, Running, Wait};
    let db = db();
    // Questions: one open with a placement and a plan, and one closed each way.
    let open_q = ask(
        &db,
        NewQuestion {
            title: "Which way for number one".into(),
            details: "details".into(),
            kind: QuestionKind::YourStep,
            blocks_nothing: None,
            placed: Some(Placement { at: Position::First, why: "person said".into(), by_person: true }),
            plan: Some(PlanRef { path: "plans/p.md".into(), number: "Q3".into() }),
        },
        &stamp(NOON + 1),
    )
    .unwrap();
    let answered_q = ask(
        &db,
        NewQuestion {
            title: "Which way for number two".into(),
            details: "details".into(),
            kind: QuestionKind::Question,
            blocks_nothing: Some("asked anyway".into()),
            placed: None,
            plan: Some(PlanRef { path: "plans/p.md".into(), number: "Q4".into() }),
        },
        &stamp(NOON + 2),
    )
    .unwrap();
    let settled_q = question(&db);
    let done_q = question(&db);
    let dropped_q = question(&db);
    let deferred_q = question(&db);
    // A decision answering one question and making an item; a second replacing it, with a plan.
    let first = add_decision(
        &db,
        &NewDecision {
            title: "Chose the first way".into(),
            by: "person".into(),
            body: "why".into(),
            question: Some(answered_q),
            replaces: None,
            plan: Some(PlanRef { path: "plans/p.md".into(), number: "Q4".into() }),
        },
        &[draft("Build the first way now")],
        &stamp(NOON + 10),
    )
    .unwrap();
    close(&db, answered_q, crate::lists::How::Answered(first.decision), &stamp(NOON + 11)).unwrap();
    add_decision(
        &db,
        &NewDecision {
            title: "Chose the second way".into(),
            by: "design-session".into(),
            body: "because".into(),
            question: None,
            replaces: Some(first.decision),
            plan: Some(PlanRef { path: "plans/q.md".into(), number: "D1".into() }),
        },
        &[],
        &stamp(NOON + 12),
    )
    .unwrap();
    close(&db, settled_q, crate::lists::How::Settled("the rule".into()), &stamp(NOON + 13)).unwrap();
    close(&db, done_q, crate::lists::How::Done, &stamp(NOON + 14)).unwrap();
    close(&db, dropped_q, crate::lists::How::Dropped("no".into()), &stamp(NOON + 15)).unwrap();
    close(&db, deferred_q, crate::lists::How::Deferred("https://x/1".into()), &stamp(NOON + 16)).unwrap();
    // Items: one open that waits four ways, runs in two repos and has notes, a part landed and a priority.
    let design_id = design(&db);
    let blocker = item(&db, vec![]);
    let busy = item(
        &db,
        vec![Wait::Item(blocker), Wait::Question(open_q), Wait::Design(design_id), Wait::Condition("a quiet machine".into())],
    );
    start(
        &db,
        &[busy],
        &[
            Running { worktree: "/d/bae-worktrees/side-2".into(), branch: "b".into(), brief: "plans/b.md".into(), since: stamp(NOON + 20) },
            Running { worktree: "/d/coven-worktrees/side-1".into(), branch: "c".into(), brief: "plans/b.md".into(), since: stamp(NOON + 20) },
        ],
    )
    .unwrap();
    add_note(&db, busy, Note { at: NOON + 21, by: "agent".into(), text: "halfway".into() }).unwrap();
    add_note(&db, busy, Note { at: NOON + 22, by: "agent".into(), text: "nearly".into() }).unwrap();
    add_part(&db, busy, LandedPart { repo: "bae".into(), commits: vec![sha('1'), sha('2')], at: NOON + 23 }).unwrap();
    set_priority(&db, busy, Some(Priority { level: Level::First, why: "person".into() })).unwrap();
    // One closed each way.
    let landed = item(&db, vec![]);
    add_part(&db, landed, LandedPart { repo: "coven".into(), commits: vec![sha('3')], at: NOON + 24 }).unwrap();
    close(&db, landed, crate::lists::How::Landed(vec![sha('a'), sha('b')]), &stamp(NOON + 25)).unwrap();
    let dropped = item(&db, vec![]);
    close(&db, dropped, crate::lists::How::Dropped("gone".into()), &stamp(NOON + 26)).unwrap();
    let deferred = item(&db, vec![]);
    close(&db, deferred, crate::lists::How::Deferred("https://x/2".into()), &stamp(NOON + 27)).unwrap();
    let moved = item(&db, vec![]);
    close(&db, moved, crate::lists::How::MovedToStack(open_q), &stamp(NOON + 28)).unwrap();
    // The design change: places by item and by hand, a board with its link, a review page, an approval.
    let mut change = read(&db).design(design_id).unwrap().clone();
    change.places.insert("design/mocks/A.dc.html".into(), Resolution::Item(busy));
    change.places.insert("design/system/components/B/".into(), Resolution::Matches("diffed".into()));
    change.boards.insert("design/mocks/A.dc.html".into(), Board { link: "https://canvas.example/#A".into() });
    change.review = vec![
        ReviewSection {
            heading: "What changed".into(),
            words: "words".into(),
            boards: vec![ReviewBoard { board: "design/mocks/A.dc.html".into(), caption: "A".into(), theme: Theme::Light }],
        },
        ReviewSection { heading: "Second".into(), words: String::new(), boards: vec![] },
    ];
    save(&db, &change).unwrap();
    crate::lists::design::approve(&db, design_id, &stamp(NOON + 30)).unwrap();
    let landed_design = design(&db);
    close(&db, landed_design, crate::lists::How::Landed(vec![sha('d')]), &stamp(NOON + 31)).unwrap();
    // Waits on closed records that meet them.
    add_wait(&db, busy, Wait::Item(landed)).unwrap();
    add_wait(&db, busy, Wait::Question(answered_q)).unwrap();
    add_wait(&db, busy, Wait::Design(landed_design)).unwrap();
    assert_eq!(crate::lists::check::faults(&db, "demo").unwrap(), Vec::<String>::new());
    db
}

/// What `lists.json` held for <db>: the lists as the JSON store wrote them,
/// with its counter and log.
fn json_of(db: &Connection) -> serde_json::Value {
    let lists = crate::lists::store::read(db).unwrap();
    let mut value = serde_json::to_value(&lists).unwrap();
    let top = db.query_row("SELECT MAX(id) FROM record", [], |row| row.get::<_, i64>(0)).unwrap();
    value["next_id"] = serde_json::json!(top + 1);
    value["log"] = crate::lists::store::read_log(db)
        .unwrap()
        .into_iter()
        .map(|(version, at, by, words)| serde_json::json!({"version": version, "at": at, "by": by, "words": words}))
        .collect();
    value
}

/// A project folder `demo` holding <value> as `lists.json`.
fn folder_with(scratch: &Scratch, value: &serde_json::Value) -> (Places, std::path::PathBuf) {
    let places = scratch.places();
    let dir = places.lists.join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lists.json"), serde_json::to_string_pretty(value).unwrap()).unwrap();
    // The lock every old binary's write held.
    std::fs::write(dir.join("lists.lock"), "").unwrap();
    (places, dir)
}

fn names_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn moved(places: &Places) -> crate::lists::Lists {
    crate::lists::store::Store::named(places, "demo").unwrap().load().unwrap()
}

#[test]
fn every_row_kind_moves_and_reads_back_as_the_same_lists_one_version_higher() {
    let source = full_lists();
    let mut value = json_of(&source);
    // A closed item that still lists where it ran, as the live file's 31 do.
    let closed_item = value["closed"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find_map(|c| c.pointer_mut("/record/item"))
        .unwrap();
    closed_item["running"] = serde_json::json!([{"worktree": "/d/bae-worktrees/side-9", "branch": "x", "brief": "b", "since": {"at": 1, "by": "a"}}]);
    let scratch = Scratch::new("move-all");
    let (places, dir) = folder_with(&scratch, &value);
    migrate_lists(&places, 5_000).unwrap();
    let mut expected = crate::lists::store::read(&source).unwrap();
    expected.version += 1;
    assert_eq!(moved(&places), expected);
    // The log is the file's, and the move's own line.
    let db = crate::db::open_reader(&dir.join(DATABASE)).unwrap().unwrap();
    let log = crate::lists::store::read_log(&db).unwrap();
    assert_eq!(log.len() as u64, expected.version);
    assert_eq!(log.last().unwrap(), &(expected.version, 5_000, "builds install".to_string(), "move lists.json into builds.sqlite".to_string()));
    assert_eq!(log[..log.len() - 1], crate::lists::store::read_log(&source).unwrap()[..]);
}

#[test]
fn after_the_move_the_json_is_kept_as_moved_and_the_lock_is_gone() {
    let source = full_lists();
    let scratch = Scratch::new("move-files");
    let (places, dir) = folder_with(&scratch, &json_of(&source));
    migrate_lists(&places, 5_000).unwrap();
    let names = names_in(&dir);
    assert!(names.contains(&"lists.json.moved".to_string()), "{names:?}");
    for gone in ["lists.json", "lists.json.moving", "lists.lock"] {
        assert!(!names.contains(&gone.to_string()), "{gone} is still there: {names:?}");
    }
    assert!(names.contains(&DATABASE.to_string()));
}

#[test]
fn a_second_install_changes_nothing() {
    let source = full_lists();
    let scratch = Scratch::new("move-again");
    let (places, dir) = folder_with(&scratch, &json_of(&source));
    migrate_lists(&places, 5_000).unwrap();
    let before = (moved(&places), names_in(&dir));
    migrate_lists(&places, 9_000).unwrap();
    assert_eq!((moved(&places), names_in(&dir)), before);
}

#[test]
fn a_move_that_crashed_after_the_rename_is_finished_by_running_it_again() {
    let source = full_lists();
    let scratch = Scratch::new("move-crash-renamed");
    let (places, dir) = folder_with(&scratch, &json_of(&source));
    std::fs::rename(dir.join("lists.json"), dir.join("lists.json.moving")).unwrap();
    // Until it is finished, every command refuses and names the move.
    let refused = crate::lists::store::Store::named(&places, "demo").unwrap().load().unwrap_err().text();
    assert!(refused.contains("mid-move") && refused.contains("builds install"), "{refused}");
    migrate_lists(&places, 5_000).unwrap();
    assert_eq!(moved(&places).version, crate::lists::store::version(&source).unwrap() + 1);
    assert!(dir.join("lists.json.moved").exists() && !dir.join("lists.json.moving").exists());
}

#[test]
fn a_move_that_crashed_after_the_database_commit_only_renames() {
    let source = full_lists();
    let scratch = Scratch::new("move-crash-committed");
    let (places, dir) = folder_with(&scratch, &json_of(&source));
    migrate_lists(&places, 5_000).unwrap();
    let before = moved(&places);
    std::fs::rename(dir.join("lists.json.moved"), dir.join("lists.json.moving")).unwrap();
    std::fs::write(dir.join("lists.lock"), "").unwrap();
    migrate_lists(&places, 9_000).unwrap();
    assert_eq!(moved(&places), before, "the rows are not written twice");
    assert!(dir.join("lists.json.moved").exists() && !dir.join("lists.json.moving").exists());
}

#[test]
fn a_moving_file_beside_lists_at_another_version_is_refused_naming_both() {
    let source = full_lists();
    let scratch = Scratch::new("move-crash-other");
    let (places, dir) = folder_with(&scratch, &json_of(&source));
    migrate_lists(&places, 5_000).unwrap();
    let store = crate::lists::store::Store::named(&places, "demo").unwrap();
    store.write(&crate::lists::fixtures::stamp(6_000), "area add Gamma", |db| crate::lists::area::add_area(db, "Gamma", None)).unwrap();
    let before = moved(&places);
    std::fs::rename(dir.join("lists.json.moved"), dir.join("lists.json.moving")).unwrap();
    let refused = migrate_lists(&places, 9_000).unwrap_err().text();
    assert!(refused.contains("lists.json.moving") && refused.contains(&format!("version {}", before.version)), "{refused}");
    assert!(dir.join("lists.json.moving").exists());
    assert_eq!(moved_after_rename_back(&dir, &places), before);
}

/// <dir>'s `lists.json.moving` back as `lists.json.moved`, then the store as it reads.
fn moved_after_rename_back(dir: &std::path::Path, places: &Places) -> crate::lists::Lists {
    std::fs::rename(dir.join("lists.json.moving"), dir.join("lists.json.moved")).unwrap();
    moved(places)
}

#[test]
fn json_beside_lists_rows_and_both_json_files_are_refused() {
    let source = full_lists();
    let scratch = Scratch::new("move-beside");
    let (places, dir) = folder_with(&scratch, &json_of(&source));
    migrate_lists(&places, 5_000).unwrap();
    std::fs::copy(dir.join("lists.json.moved"), dir.join("lists.json")).unwrap();
    let refused = migrate_lists(&places, 9_000).unwrap_err().text();
    assert!(refused.contains("lists.json") && refused.contains("beside lists rows"), "{refused}");
    std::fs::copy(dir.join("lists.json.moved"), dir.join("lists.json.moving")).unwrap();
    let refused = migrate_lists(&places, 9_000).unwrap_err().text();
    assert!(refused.contains("both lists.json and lists.json.moving"), "{refused}");
}

/// Moves <value> and returns the refusal, with the folder checked as it was.
fn refused_move(name: &str, value: &serde_json::Value) -> String {
    let scratch = Scratch::new(name);
    let (places, dir) = folder_with(&scratch, value);
    let before = std::fs::read(dir.join("lists.json")).unwrap();
    let refused = migrate_lists(&places, 5_000).unwrap_err().text();
    assert_eq!(std::fs::read(dir.join("lists.json")).unwrap(), before, "lists.json is as it was");
    let names = names_in(&dir);
    assert!(!names.contains(&"lists.json.moving".to_string()) && !names.contains(&DATABASE.to_string()), "{names:?}");
    refused
}

#[test]
fn a_json_that_cannot_move_is_refused_and_both_files_are_as_they_were() {
    let source = full_lists();
    let good = json_of(&source);
    // A title the database refuses.
    let mut bad = good.clone();
    bad["items"][0]["title"] = serde_json::json!("two\nlines");
    assert!(refused_move("move-bad-title", &bad).contains("item_title_is_one_line"));
    // A shape that is not today's.
    let mut old = good.clone();
    old["items"][0]["running"] = serde_json::json!({"worktree": "/d/bae-worktrees/side-1", "branch": "b", "brief": "b", "since": {"at": 1, "by": "a"}});
    assert!(refused_move("move-old-shape", &old).contains("not today's shape"));
    // A wait that can no longer be met.
    let mut dropped_wait = good.clone();
    dropped_wait["closed"][0]["how"] = serde_json::json!({"dropped": "no"});
    let refused = refused_move("move-unmeetable", &dropped_wait);
    assert!(!refused.is_empty());
}

#[test]
fn each_derived_field_that_disagrees_with_what_it_is_derived_from_is_refused() {
    let source = full_lists();
    let good = json_of(&source);
    type Case = (&'static str, Box<dyn Fn(&mut serde_json::Value)>, &'static str);
    let cases: Vec<Case> = vec![
        ("next-id", Box::new(|v| v["next_id"] = serde_json::json!(3)), "next_id is 3"),
        ("version", Box::new(|v| v["version"] = serde_json::json!(99)), "version is 99"),
        ("question", Box::new(|v| v["decisions"][0]["question"] = serde_json::Value::Null), "says it answered None"),
        ("work", Box::new(|v| v["decisions"][0]["work"] = serde_json::json!([])), "says it made []"),
        (
            "landed",
            Box::new(|v| {
                let design = v["closed"].as_array_mut().unwrap().iter_mut().find_map(|c| c.pointer_mut("/record/design")).unwrap();
                design["landed"] = serde_json::json!([]);
            }),
            "says it landed as []",
        ),
        (
            "briefs",
            Box::new(|v| {
                let item = v["items"].as_array_mut().unwrap().iter_mut().find(|i| i["running"].as_array().is_some_and(|r| r.len() == 2)).unwrap();
                item["running"][1]["brief"] = serde_json::json!("other.md");
            }),
            "different briefs or moments",
        ),
        ("plan", Box::new(|v| v["decisions"][0]["plan"] = serde_json::Value::Null), "has a plan other than its question"),
    ];
    for (name, change, expected) in cases {
        let mut bad = good.clone();
        change(&mut bad);
        let refused = refused_move(&format!("move-derived-{name}"), &bad);
        assert!(refused.contains(expected), "{name}: {refused}");
    }
}

/// A database the first hook stats made: only their tables, under their first stamp, holding one run.
fn hook_only_database(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(include_str!("hook_stats.sql")).unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    conn.execute(
        "INSERT INTO hook_runs (started_at, repo, worktree, passed, plan_ms, total_ms) VALUES (5, 'bae', '/d/bae', 1, 1, 9)",
        [],
    )
    .unwrap();
}

#[test]
fn a_database_holding_only_hook_runs_keeps_them_and_gains_the_lists_tables() {
    let source = full_lists();
    let scratch = Scratch::new("move-hooks");
    let (places, dir) = folder_with(&scratch, &json_of(&source));
    hook_only_database(&dir.join(DATABASE));
    migrate_lists(&places, 5_000).unwrap();
    let mut expected = crate::lists::store::read(&source).unwrap();
    expected.version += 1;
    assert_eq!(moved(&places), expected);
    let stats = crate::hook_stats::stats(&dir.join(DATABASE), 0).unwrap().unwrap();
    assert_eq!(stats.runs, 1, "the hook run is kept");
    let conn = Connection::open(dir.join(DATABASE)).unwrap();
    let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0)).unwrap();
    assert_eq!(mode, "wal", "a database made by the hook stats alone is moved to WAL too");
}

#[test]
fn a_folder_with_only_hook_stats_and_no_json_gets_the_lists_tables_too() {
    let scratch = Scratch::new("move-hooks-only");
    let places = scratch.places();
    let dir = places.lists.join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    hook_only_database(&dir.join(DATABASE));
    migrate_lists(&places, 5_000).unwrap();
    let conn = crate::db::open_reader(&dir.join(DATABASE)).unwrap().expect("opens under the schema's stamp");
    assert_eq!(crate::db::schema_rows(&conn).unwrap(), crate::db::schema_rows(&crate::db::memory()).unwrap());
    assert_eq!(crate::hook_stats::stats(&dir.join(DATABASE), 0).unwrap().unwrap().runs, 1);
}

#[test]
fn a_hook_database_whose_schema_is_not_a_fresh_ones_is_refused_and_left_as_it_was() {
    let source = full_lists();
    let scratch = Scratch::new("move-hooks-drift");
    let (places, dir) = folder_with(&scratch, &json_of(&source));
    hook_only_database(&dir.join(DATABASE));
    Connection::open(dir.join(DATABASE)).unwrap().execute_batch("DROP INDEX idx_hook_runs_started").unwrap();
    let refused = migrate_lists(&places, 5_000).unwrap_err().text();
    assert!(refused.contains("not the one this builds makes"), "{refused}");
    let conn = Connection::open(dir.join(DATABASE)).unwrap();
    let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name = 'project'", [], |r| r.get(0)).unwrap();
    assert_eq!(tables, 0, "the lists' tables were rolled back");
    assert_eq!(db::stamp_of(&conn, &dir.join(DATABASE)).unwrap(), 1);
}
