use super::*;
use crate::lists::decision::{NewDecision, add_decision};
use crate::lists::fixtures::{NOON, draft, lists, stamp};
use crate::testing::{Scratch, repo};
use std::collections::BTreeMap;

fn fresh(scratch: &Scratch) -> Store {
    let store = Store::named(&scratch.places(), "demo").unwrap();
    let mut state = Lists::empty("demo", vec!["bae".into(), "coven".into()], BTreeMap::new());
    state.areas = vec!["Alpha".into()];
    store.create(state, &stamp(NOON), "lists init demo").unwrap();
    store
}

fn bytes(store: &Store) -> Vec<u8> {
    std::fs::read(store.state_path()).unwrap()
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

#[test]
fn create_writes_the_first_version_and_load_reads_it_back() {
    let scratch = Scratch::new("lists-create");
    let store = fresh(&scratch);
    let loaded = store.load().unwrap();
    assert_eq!((loaded.version, loaded.log.len()), (1, 1));
    assert_eq!(loaded.log[0].words, "lists init demo");
    assert!(bytes(&store).ends_with(b"}\n"));
}

#[test]
fn init_refuses_an_existing_store_and_leaves_it() {
    let scratch = Scratch::new("lists-twice");
    let store = fresh(&scratch);
    let before = bytes(&store);
    let again = Lists::empty("demo", vec!["bae".into()], BTreeMap::new());
    assert!(message(store.create(again, &stamp(NOON + 9), "lists init demo")).contains("already exist"));
    assert_eq!(bytes(&store), before);
}

#[test]
fn init_refuses_a_state_that_fails_the_check_and_writes_nothing() {
    let scratch = Scratch::new("lists-bad-init");
    let store = Store::named(&scratch.places(), "demo").unwrap();
    let bad = Lists::empty("demo", vec![], BTreeMap::new());
    assert!(message(store.create(bad, &stamp(NOON), "lists init demo")).contains("no repos"));
    assert!(!store.exists());
}

#[test]
fn a_write_applies_logs_and_bumps_the_version() {
    let scratch = Scratch::new("lists-write");
    let store = fresh(&scratch);
    let area = store
        .with_write(&stamp(NOON + 5), "area add Beta", |lists| {
            lists.areas.push("Beta".into());
            Ok(lists.areas.len())
        })
        .unwrap();
    assert_eq!(area, 2);
    let loaded = store.load().unwrap();
    assert_eq!((loaded.version, loaded.areas.clone()), (2, vec!["Alpha".to_string(), "Beta".to_string()]));
    let line = loaded.log.last().unwrap();
    assert_eq!((line.version, line.at, line.by.as_str(), line.words.as_str()), (2, NOON + 5, "coordinator", "area add Beta"));
    assert_eq!(names_in(&store.dir), vec!["lists.json", "lists.lock"]);
}

#[test]
fn a_refused_command_leaves_the_file_byte_for_byte() {
    let scratch = Scratch::new("lists-refused");
    let store = fresh(&scratch);
    let before = bytes(&store);
    // The command changes the state, then refuses.
    let result: Result<()> = store.with_write(&stamp(NOON + 1), "words", |lists| {
        lists.areas.push("Half".into());
        fail!("refused after a change")
    });
    assert!(message(result).contains("refused after a change"));
    assert_eq!(bytes(&store), before);
    assert_eq!(names_in(&store.dir), vec!["lists.json", "lists.lock"]);
}

#[test]
fn a_write_that_fails_the_whole_state_check_changes_nothing() {
    let scratch = Scratch::new("lists-check-refuses");
    let store = fresh(&scratch);
    let before = bytes(&store);
    let result = store.with_write(&stamp(NOON + 1), "words", |lists| {
        lists.areas.push("Alpha".into());
        Ok(())
    });
    assert!(message(result).contains("'Alpha' is listed twice"));
    assert_eq!(bytes(&store), before);
    assert_eq!(names_in(&store.dir), vec!["lists.json", "lists.lock"]);
}

#[test]
fn a_decision_whose_second_item_is_refused_lands_nothing_of_the_first() {
    let scratch = Scratch::new("lists-atomic");
    let store = fresh(&scratch);
    let before = bytes(&store);
    let new = NewDecision {
        title: "A decision".into(),
        by: "person".into(),
        body: "why".into(),
        question: None,
        replaces: None,
        plan: None,
    };
    let work = [draft("One two three four five"), draft("Short")];
    let result = store.with_write(&stamp(NOON + 1), "decision add", |lists| add_decision(lists, &new, &work, &stamp(NOON + 1)));
    assert!(message(result).contains("1 words"));
    assert_eq!(bytes(&store), before);
}

#[test]
fn a_store_failing_the_check_makes_load_fail_naming_the_fault() {
    let scratch = Scratch::new("lists-corrupt");
    let store = fresh(&scratch);
    let mut state = lists();
    crate::lists::fixtures::question(&mut state);
    state.questions[0].blocks_nothing = None;
    std::fs::write(store.state_path(), serde_json::to_string_pretty(&state).unwrap()).unwrap();
    let text = message(store.load());
    assert!(text.contains("fail their check") && text.contains("blocks no open item"), "{text}");
    // A write refuses too, and changes nothing.
    let before = bytes(&store);
    assert!(message(store.with_write(&stamp(NOON), "w", |_| Ok(()))).contains("blocks no open item"));
    assert_eq!(bytes(&store), before);
}

#[test]
fn unreadable_or_foreign_state_is_refused() {
    let scratch = Scratch::new("lists-foreign");
    let store = fresh(&scratch);
    std::fs::write(store.state_path(), "{ not json").unwrap();
    assert!(message(store.load()).contains("is not valid lists"));
    let mut state = Lists::empty("other", vec!["bae".into()], BTreeMap::new());
    state.version = 0;
    std::fs::write(store.state_path(), serde_json::to_string(&state).unwrap()).unwrap();
    assert!(message(store.load()).contains("holds the lists of project other, not demo"));
    let mut extra: serde_json::Value = serde_json::to_value(lists()).unwrap();
    extra["surprise"] = serde_json::json!(1);
    std::fs::write(store.state_path(), extra.to_string()).unwrap();
    assert!(message(store.load()).contains("surprise"));
}

#[test]
fn a_missing_store_says_how_to_make_it_and_a_write_makes_no_folder() {
    let scratch = Scratch::new("lists-missing");
    let store = Store::named(&scratch.places(), "demo").unwrap();
    assert!(message(store.load()).contains("builds lists init demo"));
    assert!(message(store.with_write(&stamp(NOON), "w", |_| Ok(()))).contains("builds lists init demo"));
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
    store.with_write(&stamp(NOON), "w", |lists| {
        lists.areas.push("Beta".into());
        Ok(())
    })
    .unwrap();
    assert_eq!(names_in(&places.lists), vec!["demo"]);
    assert_eq!(names_in(&places.lists.join("demo")), vec!["lists.json", "lists.lock"]);
    assert_eq!(names_in(&scratch.dir), vec!["lists", "tmp", "volume"]);
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
            .with_write(&stamp(NOON + i as i64), &format!("area add {who}-{i}"), |lists| {
                lists.areas.push(format!("{who}-{i}"));
                Ok(())
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
    assert_eq!(loaded.areas.len(), 1 + 2 * WRITES);
    assert_eq!(loaded.version, 1 + 2 * WRITES as u64);
    assert_eq!(loaded.log.len(), 1 + 2 * WRITES);
    for who in ["a", "b"] {
        let mine: Vec<&String> = loaded.areas.iter().filter(|area| area.starts_with(who)).collect();
        let expected: Vec<String> = (0..WRITES).map(|i| format!("{who}-{i}")).collect();
        assert_eq!(mine, expected.iter().collect::<Vec<_>>(), "{who}'s writes keep their order");
    }
    assert_eq!(names_in(&store.dir), vec!["lists.json", "lists.lock"]);
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
    second.create(Lists::empty("second", vec!["bae".into()], BTreeMap::new()), &stamp(NOON), "lists init second").unwrap();
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
