use super::*;
use crate::lists::decision::{NewDecision, add_decision};
use crate::lists::fixtures::{NOON, draft, make_store, stamp};
use crate::testing::{Scratch, repo};

fn fresh(scratch: &Scratch) -> Store {
    make_store(&scratch.places(), &["bae", "coven"])
}

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// Everything the store holds: the lists as read and the write log.
fn held(store: &Store) -> String {
    let db = db::open_reader(&store.database()).unwrap().expect("a store");
    format!("{:?}\n{:?}", read(&db).unwrap(), read_log(&db).unwrap())
}

fn new_project(repos: &[&str]) -> NewProject {
    NewProject {
        name: "demo".into(),
        repos: repos.iter().map(|r| r.to_string()).collect(),
        places: Default::default(),
        sources: Default::default(),
        specs: Default::default(),
    }
}

#[test]
fn create_writes_the_first_version_and_load_reads_it_back() {
    let scratch = Scratch::new("lists-create");
    let store = Store::named(&scratch.places(), "demo").unwrap();
    store.create(&new_project(&["bae", "coven"]), &stamp(NOON), "lists init demo").unwrap();
    let loaded = store.load().unwrap();
    assert_eq!((loaded.version, loaded.repos.clone()), (1, vec!["bae".to_string(), "coven".to_string()]));
    let db = db::open_reader(&store.database()).unwrap().unwrap();
    assert_eq!(read_log(&db).unwrap(), vec![(1, NOON, "coordinator".to_string(), "lists init demo".to_string())]);
    assert_eq!(db::stamp_of(&db, &store.database()).unwrap(), db::stamp());
}

#[test]
fn init_refuses_an_existing_store_and_leaves_it() {
    let scratch = Scratch::new("lists-twice");
    let store = fresh(&scratch);
    let before = held(&store);
    assert!(message(store.create(&new_project(&["bae"]), &stamp(NOON + 9), "lists init demo")).contains("already exist"));
    assert_eq!(held(&store), before);
}

#[test]
fn init_refuses_a_state_that_fails_the_check_and_makes_no_project() {
    let scratch = Scratch::new("lists-bad-init");
    let store = Store::named(&scratch.places(), "demo").unwrap();
    assert!(message(store.create(&new_project(&[]), &stamp(NOON), "lists init demo")).contains("no repos"));
    assert!(message(store.load()).contains("builds lists init demo"));
    // The same init, repaired, makes the store.
    store.create(&new_project(&["bae"]), &stamp(NOON), "lists init demo").unwrap();
    assert_eq!(store.load().unwrap().version, 1);
}

#[test]
fn a_write_commits_whole_with_its_log_line_and_the_next_version() {
    let scratch = Scratch::new("lists-write");
    let store = fresh(&scratch);
    let before = store.load().unwrap();
    let areas = store
        .write(&stamp(NOON + 5), "area add Gamma", |db| {
            crate::lists::area::add_area(db, "Gamma", None)?;
            Ok(crate::lists::area::areas(db)?.len())
        })
        .unwrap();
    assert_eq!(areas, 3);
    let loaded = store.load().unwrap();
    assert_eq!((loaded.version, loaded.areas.clone()), (before.version + 1, vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()]));
    let db = db::open_reader(&store.database()).unwrap().unwrap();
    let line = read_log(&db).unwrap().pop().unwrap();
    assert_eq!(line, (loaded.version, NOON + 5, "coordinator".to_string(), "area add Gamma".to_string()));
    assert_eq!(names_in(&store.dir), vec!["builds.sqlite", "builds.sqlite-shm", "builds.sqlite-wal"]);
}

#[test]
fn a_refused_command_changes_no_table_and_not_the_version() {
    let scratch = Scratch::new("lists-refused");
    let store = fresh(&scratch);
    let before = held(&store);
    // The command changes the state, then refuses.
    let result: Result<()> = store.write(&stamp(NOON + 1), "words", |db| {
        crate::lists::area::add_area(db, "Half", None)?;
        fail!("refused after a change")
    });
    assert!(message(result).contains("refused after a change"));
    assert_eq!(held(&store), before);
}

#[test]
fn a_write_that_fails_the_code_check_changes_nothing() {
    let scratch = Scratch::new("lists-check-refuses");
    let store = fresh(&scratch);
    let before = held(&store);
    // A question that blocks nothing and says nothing of why.
    let result = store.write(&stamp(NOON + 1), "words", |db| {
        let id = crate::lists::fixtures::question(db);
        crate::lists::fixtures::blocks_something(db, id);
        Ok(())
    });
    assert!(message(result).contains("blocks no open item"));
    assert_eq!(held(&store), before);
}

#[test]
fn a_write_the_database_refuses_changes_nothing() {
    let scratch = Scratch::new("lists-constraint");
    let store = fresh(&scratch);
    let before = held(&store);
    // The second item of a decision is refused after the first is written.
    let new = NewDecision {
        title: "A decision".into(),
        by: "person".into(),
        body: "why".into(),
        question: None,
        replaces: None,
        plan: None,
    };
    let work = [draft("One two three four five"), draft("Short")];
    let result = store.write(&stamp(NOON + 1), "decision add", |db| add_decision(db, &new, &work, &stamp(NOON + 1)));
    assert!(message(result).contains("1 words"));
    assert_eq!(held(&store), before);
    // A constraint of the table, hit by a write that skipped the owner's own check.
    let result = store.write(&stamp(NOON + 1), "words", |db| {
        crate::lists::area::plant(db, &[("name", "Alpha".to_string().into()), ("position", 9_i64.into())]).map(|_| ()).sql()
    });
    assert!(message(result).contains("UNIQUE constraint failed: area.name"));
    assert_eq!(held(&store), before);
}

#[test]
fn a_reader_sees_the_last_committed_snapshot_during_a_write() {
    let scratch = Scratch::new("lists-snapshot");
    let store = fresh(&scratch);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let writer = {
        let store = store.clone();
        std::thread::spawn(move || {
            store
                .write(&stamp(NOON + 1), "area add Gamma", |db| {
                    crate::lists::area::add_area(db, "Gamma", None)?;
                    entered_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        })
    };
    entered_rx.recv().unwrap();
    // The write holds the write lock with its change uncommitted.
    let other = Connection::open_with_flags(store.database(), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
    other.busy_timeout(std::time::Duration::ZERO).unwrap();
    assert!(other.execute_batch("BEGIN IMMEDIATE").is_err(), "the write lock is held");
    let seen = store.load().unwrap();
    assert_eq!((seen.version, seen.areas.len()), (3, 2), "the reader sees what was committed before the write");
    go_tx.send(()).unwrap();
    writer.join().unwrap();
    let after = store.load().unwrap();
    assert_eq!((after.version, after.areas.len()), (4, 3));
}

#[test]
fn a_second_write_waits_for_the_first_to_commit_and_then_sees_it() {
    let scratch = Scratch::new("lists-turns");
    let store = fresh(&scratch);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let first = {
        let store = store.clone();
        std::thread::spawn(move || {
            store
                .write(&stamp(NOON + 1), "area add Gamma", |db| {
                    crate::lists::area::add_area(db, "Gamma", None)?;
                    entered_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        })
    };
    entered_rx.recv().unwrap();
    let second = {
        let store = store.clone();
        std::thread::spawn(move || {
            // Only reached once the first has committed.
            store.write(&stamp(NOON + 2), "area add Delta", crate::lists::area::areas).unwrap()
        })
    };
    go_tx.send(()).unwrap();
    first.join().unwrap();
    assert_eq!(second.join().unwrap(), vec!["Alpha", "Beta", "Gamma"]);
    assert_eq!(store.load().unwrap().version, 5);
}

#[test]
fn a_database_of_another_schema_is_refused_by_every_open() {
    let scratch = Scratch::new("lists-stamp");
    let store = fresh(&scratch);
    let other = Connection::open(store.database()).unwrap();
    other.pragma_update(None, "user_version", 99).unwrap();
    drop(other);
    assert!(message(store.load()).contains("made by another builds schema"));
    assert!(message(store.write(&stamp(NOON), "w", |_| Ok(()))).contains("made by another builds schema"));
    assert!(message(store.revision()).contains("made by another builds schema"));
}

#[test]
fn lists_still_in_json_are_refused_by_every_open_naming_the_move() {
    let scratch = Scratch::new("lists-json-beside");
    let store = fresh(&scratch);
    std::fs::write(store.dir.join(JSON_STATE), "{}").unwrap();
    let text = message(store.load());
    assert!(text.contains("still in lists.json") && text.contains("builds install"), "{text}");
    assert!(message(store.write(&stamp(NOON), "w", |_| Ok(()))).contains("builds install"));
    std::fs::rename(store.dir.join(JSON_STATE), store.dir.join(JSON_MOVING)).unwrap();
    let text = message(store.load());
    assert!(text.contains("mid-move") && text.contains("builds install"), "{text}");
    assert!(message(store.create(&new_project(&["bae"]), &stamp(NOON), "w")).contains("mid-move"));
}

#[test]
fn a_database_of_another_project_is_refused() {
    let scratch = Scratch::new("lists-foreign");
    let store = fresh(&scratch);
    let other = Store::named(&scratch.places(), "other").unwrap();
    std::fs::create_dir_all(&other.dir).unwrap();
    std::fs::copy(store.database(), other.database()).unwrap();
    assert!(message(other.load()).contains("holds the lists of project demo, not other"));
}

#[test]
fn a_missing_store_says_how_to_make_it_and_a_write_makes_no_folder() {
    let scratch = Scratch::new("lists-missing");
    let store = Store::named(&scratch.places(), "demo").unwrap();
    assert!(message(store.load()).contains("builds lists init demo"));
    assert!(message(store.write(&stamp(NOON), "w", |_| Ok(()))).contains("builds lists init demo"));
    assert!(!store.dir.exists());
}

#[test]
fn a_project_name_cannot_leave_the_store_folder() {
    let scratch = Scratch::new("lists-names");
    let places = scratch.places();
    for bad in ["../x", "a/b", "..", ".", "", ".hidden"] {
        assert!(Store::named(&places, bad).is_err(), "{bad}");
    }
}

#[test]
fn the_store_touches_only_its_own_folder() {
    let scratch = Scratch::new("lists-confined");
    let places = scratch.places();
    let store = fresh(&scratch);
    store.write(&stamp(NOON), "w", |db| crate::lists::area::add_area(db, "Gamma", None)).unwrap();
    assert_eq!(names_in(&places.lists), vec!["demo"]);
    assert_eq!(names_in(&scratch.dir), vec!["lists", "tmp", "volume"]);
}

#[test]
fn a_folder_with_only_hook_stats_is_no_store_and_one_with_json_is() {
    let scratch = Scratch::new("lists-stores");
    let places = scratch.places();
    let hooks = places.lists.join("hooks-only");
    std::fs::create_dir_all(&hooks).unwrap();
    db::open(&hooks.join(DATABASE), Make::Allowed).unwrap();
    let json = places.lists.join("json");
    std::fs::create_dir_all(&json).unwrap();
    std::fs::write(json.join(JSON_STATE), "{}").unwrap();
    fresh(&scratch);
    let names: Vec<String> = stores(&places).unwrap().into_iter().map(|s| s.project).collect();
    assert_eq!(names, vec!["demo", "json"]);
}

const CHILD_DIR: &str = "LISTS_TEST_CHILD_DIR";
const CHILD_WHO: &str = "LISTS_TEST_CHILD_WHO";
const CHILD_GO: &str = "LISTS_TEST_CHILD_GO";
const WRITES: usize = 40;

/// Run by `two_writers_in_two_processes_both_land` in child processes; a
/// plain run of the suite finds no environment and does nothing.
#[test]
fn two_process_child() {
    let (Some(dir), Some(who), Some(go)) = (
        std::env::var_os(CHILD_DIR),
        std::env::var(CHILD_WHO).ok(),
        std::env::var_os(CHILD_GO),
    ) else {
        return;
    };
    let store = Store { project: "demo".into(), dir: PathBuf::from(dir) };
    let started = std::time::Instant::now();
    while !Path::new(&go).exists() {
        assert!(started.elapsed().as_secs() < 60, "the parent never said go");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    for i in 0..WRITES {
        store
            .write(&stamp(NOON + i as i64), &format!("area add {who}-{i}"), |db| {
                crate::lists::area::add_area(db, &format!("{who}-{i}"), None)
            })
            .unwrap();
    }
}

#[test]
fn two_writers_in_two_processes_both_land() {
    let scratch = Scratch::new("lists-two");
    let store = fresh(&scratch);
    let go = scratch.dir.join("go");
    let spawn = |who: &str| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["two_process_child", "--nocapture", "--test-threads=1"])
            .env(CHILD_DIR, &store.dir)
            .env(CHILD_WHO, who)
            .env(CHILD_GO, &go)
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let (mut a, mut b) = (spawn("a"), spawn("b"));
    std::fs::write(&go, "").unwrap();
    assert!(a.wait().unwrap().success() && b.wait().unwrap().success());
    let loaded = store.load().unwrap();
    // Every write of both landed: none was lost to the other.
    assert_eq!(loaded.areas.len(), 2 + 2 * WRITES);
    assert_eq!(loaded.version, 3 + 2 * WRITES as u64);
    for who in ["a", "b"] {
        let mine: Vec<&String> = loaded.areas.iter().filter(|area| area.starts_with(who)).collect();
        let expected: Vec<String> = (0..WRITES).map(|i| format!("{who}-{i}")).collect();
        assert_eq!(mine, expected.iter().collect::<Vec<_>>(), "{who}'s writes keep their order");
    }
}

#[test]
fn project_of_finds_the_one_project_a_repo_is_in() {
    let scratch = Scratch::new("lists-project-of");
    let places = scratch.places();
    let repo_dir = scratch.dir.join("bae");
    repo(&repo_dir, "target 1\n");
    let checkout = Checkout::at(&repo_dir).unwrap().unwrap();
    // No lists folder at all.
    assert_eq!(project_of(&places, &checkout).unwrap(), None);
    fresh(&scratch);
    assert_eq!(project_of(&places, &checkout).unwrap(), Some("demo".to_string()));
    let elsewhere = scratch.dir.join("web");
    repo(&elsewhere, "target 1\n");
    assert_eq!(project_of(&places, &Checkout::at(&elsewhere).unwrap().unwrap()).unwrap(), None);
    // A folder with no state (an interrupted init) is not a project.
    std::fs::create_dir_all(places.lists.join("stray")).unwrap();
    assert_eq!(project_of(&places, &checkout).unwrap(), Some("demo".to_string()));
    // A repo in two projects is refused.
    let second = Store::named(&places, "second").unwrap();
    second.create(&NewProject { name: "second".into(), ..new_project(&["bae"]) }, &stamp(NOON), "lists init second").unwrap();
    assert!(message(project_of(&places, &checkout)).contains("two projects (demo, second)"));
}

#[test]
fn resolve_takes_project_flag_else_the_repo_of_the_directory() {
    let scratch = Scratch::new("lists-resolve");
    let places = scratch.places();
    let repo_dir = scratch.dir.join("coven");
    repo(&repo_dir, "target 1\n");
    fresh(&scratch);
    let none = Args::parse(&[], &[], &[]).unwrap();
    assert_eq!(Store::resolve(&places, &none, &repo_dir).unwrap().project, "demo");
    let named = Args::parse(&["--project".into(), "other".into()], &[], &[]).unwrap();
    assert_eq!(Store::resolve(&places, &named, &repo_dir).unwrap().project, "other");
    let outside = scratch.dir.join("volume");
    assert!(message(Store::resolve(&places, &none, &outside)).contains("--project"));
    let lone = scratch.dir.join("lone");
    repo(&lone, "target 1\n");
    assert!(message(Store::resolve(&places, &none, &lone)).contains("repo lone is in no project"));
}

#[test]
fn who_wrote_is_by_else_the_claim_else_refused() {
    let scratch = Scratch::new("lists-by");
    let repo_dir = scratch.dir.join("bae");
    repo(&repo_dir, "target 1\n");
    let none = Args::parse(&[], &[], &[]).unwrap();
    assert!(message(resolve_by(&none, &repo_dir)).contains("say who you are with --by"));
    assert!(message(resolve_by(&none, &scratch.dir)).contains("--by"));
    let checkout = Checkout::at(&repo_dir).unwrap().unwrap();
    Claim { who: "import-tags".into(), since: "2026-11-13 10:00".into(), note: None, note_state: None }
        .write(&checkout.claim_file())
        .unwrap();
    assert_eq!(resolve_by(&none, &repo_dir).unwrap(), "import-tags");
    let given = Args::parse(&["--by".into(), "me".into()], &[], &[]).unwrap();
    assert_eq!(resolve_by(&given, &repo_dir).unwrap(), "me");
    let blank = Args::parse(&["--by".into(), " ".into()], &[], &[]).unwrap();
    assert!(resolve_by(&blank, &repo_dir).is_err());
    assert_eq!(Stamp::resolve(7, &given, &repo_dir).unwrap(), Stamp { at: 7, by: "me".into() });
}

#[test]
fn every_named_constraint_of_the_write_log_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, row, v};
    let db = db();
    let line = [("version", v(10)), ("at", v(1)), ("by", v("a")), ("words", v("w"))];
    plant(&db, "write_log", &row(&line, &[("version", v(10))])).unwrap();
    let mut refused = Refusals::new(include_str!("store.sql"));
    refused.check("write_log_version_positive", plant(&db, "write_log", &row(&line, &[("version", v(0))])));
    refused.check("write_log_by_says_something", plant(&db, "write_log", &row(&line, &[("version", v(11)), ("by", v(" "))])));
    refused.check_as("write_log_version_once", "UNIQUE constraint failed: write_log.version", plant(&db, "write_log", &line));
    refused.covered_without_a_name(&["write_log_version_once"]);
    refused.done();
}

#[test]
fn the_integrity_checks_name_a_row_that_points_at_nothing_and_one_that_breaks_a_check() {
    let scratch = Scratch::new("lists-integrity");
    let places = scratch.places();
    let store = fresh(&scratch);
    assert_eq!(integrity(&places), Vec::<String>::new());
    // Written with the foreign keys and checks off, as the sqlite3 shell does.
    let db = db::open(&store.database(), Make::Never).unwrap();
    db.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA ignore_check_constraints = ON").unwrap();
    db.execute("INSERT INTO item_wait_item (item_id, on_item) VALUES (41, 42)", []).unwrap();
    db.execute("UPDATE area SET name = 'two\nlines' WHERE name = 'Beta'", []).unwrap();
    drop(db);
    let found = integrity(&places);
    assert!(found.iter().any(|f| f.contains("item_wait_item") && f.contains("points at nothing")), "{found:?}");
    assert!(found.iter().any(|f| f.contains("area_name_is_one_line") || f.contains("CHECK")), "{found:?}");
    assert!(found.iter().all(|f| f.contains("project demo's database fails its integrity check")), "{found:?}");
}

