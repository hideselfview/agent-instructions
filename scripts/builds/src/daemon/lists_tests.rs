use super::*;
use crate::lists::Running;
use crate::lists::fixtures::{NOON, item, lists, stamp};
use crate::lists::store::Store;
use crate::testing::Scratch;
use serde_json::json;
use std::sync::mpsc;
use std::time::Duration;

fn loaded(project: &str, lists: Result<crate::lists::Lists, String>) -> Loaded {
    Loaded { project: project.to_string(), lists }
}

fn no_claims(_: &str) -> ClaimFacts {
    panic!("no item runs")
}

fn error_of(snapshot: Snapshot) -> String {
    let view = serde_json::to_value(compose(&snapshot, NOON, &no_claims)).unwrap();
    assert_eq!(view["items"], json!([]));
    view["error"].as_str().expect("an error").to_string()
}

#[test]
fn a_store_that_fails_its_check_is_served_as_the_error() {
    let fault = error_of(Ok(vec![loaded("demo", Err("ids repeat".to_string()))]));
    assert_eq!(fault, "ids repeat");
}

#[test]
fn no_project_and_several_projects_are_errors_too() {
    assert!(error_of(Ok(vec![])).contains("no lists"));
    let several = error_of(Ok(vec![loaded("a", Ok(lists())), loaded("b", Ok(lists()))]));
    assert!(several.contains("several projects (a, b)"), "{several}");
    assert_eq!(error_of(Err("listing failed".into())), "listing failed");
}

#[test]
fn a_running_item_carries_its_claim_facts_and_nothing_else_does() {
    let mut state = lists();
    let running = item(&mut state, vec![]);
    let idle = item(&mut state, vec![]);
    let worktree = "/d/bae-worktrees/side-2";
    crate::lists::running::start(&mut state, &[running], &[Running {
        worktree: worktree.into(),
        branch: "import-tags".into(),
        brief: "plans/briefs/x.md".into(),
        since: stamp(NOON),
    }]).unwrap();
    let snapshot: Snapshot = Ok(vec![loaded("demo", Ok(state))]);
    assert_eq!(running_worktrees(&snapshot), vec![worktree.to_string()]);
    let facts = ClaimFacts {
        agent: Some("import-tags".into()),
        note: Some("tests next".into()),
        note_state: Some("held".into()),
        waiting_to_land: 2,
    };
    let view = serde_json::to_value(compose(&snapshot, NOON, &|asked| {
        assert_eq!(asked, worktree);
        facts.clone()
    }))
    .unwrap();
    let find = |id: crate::lists::Id| view["items"].as_array().unwrap().iter().find(|i| i["id"] == json!(id.0)).unwrap().clone();
    let r = &find(running)["running"][0];
    assert_eq!(
        (&r["agent"], &r["note"], &r["note_state"], &r["waiting_to_land"]),
        (&json!("import-tags"), &json!("tests next"), &json!("held"), &json!(2))
    );
    assert_eq!(find(idle)["running"], json!([]));
}

#[test]
fn a_write_to_the_store_moves_the_version_the_feed_reports() {
    let scratch = Scratch::new("daemon-lists");
    let places = scratch.places();
    let store = Store::named(&places, "demo").unwrap();
    store.create(lists(), &stamp(NOON), "lists init demo").unwrap();
    let feed = Feed::start(places);
    let (before, snapshot) = feed.current();
    assert_eq!(before, 1);
    assert_eq!(snapshot.as_ref().as_ref().unwrap()[0].lists.as_ref().unwrap().version, 2);
    let (sent, moved) = mpsc::channel();
    feed.listen(Box::new(move || sent.send(()).expect("the test waits")));

    store.with_write(&stamp(NOON), "touch", |_| Ok(())).unwrap();
    moved.recv_timeout(Duration::from_secs(30)).expect("the write reached the feed");
    let (after, snapshot) = feed.current();
    assert_eq!(after, before + 1);
    assert_eq!(snapshot.as_ref().as_ref().unwrap()[0].lists.as_ref().unwrap().version, 3);

    // A store that fails its check is read and served, not skipped.
    std::fs::write(store.state_path(), "not json").unwrap();
    moved.recv_timeout(Duration::from_secs(30)).expect("the damage reached the feed");
    let (_, snapshot) = feed.current();
    let fault = snapshot.as_ref().as_ref().unwrap()[0].lists.as_ref().unwrap_err().clone();
    assert!(fault.contains("is not valid lists"), "{fault}");
}

#[test]
fn an_open_design_change_is_found_by_id_with_its_view_and_a_landed_or_missing_one_is_not() {
    let mut state = lists();
    let id = crate::lists::fixtures::design(&mut state);
    let snapshot: Snapshot = Ok(vec![loaded("demo", Ok(state))]);
    let (design, view) = open_design(&snapshot, id.0).expect("open");
    assert_eq!((design.id, design.worktree.as_str(), view.id), (id, "/d/bae-worktrees/side-2", id));
    assert!(open_design(&snapshot, id.0 + 1).is_none());
    assert!(open_design(&Ok(vec![]), id.0).is_none());
}

#[test]
fn a_recorded_hook_run_reaches_the_feeds_hooks_json_and_moves_no_version() {
    use crate::hook_stats::{CheckRecord, DATABASE, Outcome, Run, record};
    let scratch = Scratch::new("daemon-hooks");
    let places = scratch.places();
    let store = Store::named(&places, "demo").unwrap();
    store.create(lists(), &stamp(NOON), "lists init demo").unwrap();
    let feed = Feed::start(places);
    let (version, _) = feed.current();
    let before = feed.hooks();
    assert!(before.contains("\"runs\":0"), "{before}");
    let (sent, moved) = mpsc::channel();
    feed.listen(Box::new(move || sent.send(()).expect("the test waits")));

    let run = Run {
        at: NOON,
        repo: "demo".into(),
        worktree: "/d/demo".into(),
        branch: Some("main".into()),
        passed: true,
        plan_ms: 1,
        total_ms: 5,
        files: vec![],
        checks: vec![CheckRecord {
            name: "fmt".into(),
            kind: crate::checks::Kind::Check,
            outcome: Outcome::Passed,
            handed: Some((0, 0)),
            ms: Some(3),
        }],
    };
    record(&store.dir.join(DATABASE), &run).unwrap();
    moved.recv_timeout(Duration::from_secs(30)).expect("the run reached the feed");
    let after = feed.hooks();
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    assert!(after.contains("\"runs\":1") && after.contains("\"fmt\""), "{after}");
    assert_eq!(feed.current().0, version);
}
