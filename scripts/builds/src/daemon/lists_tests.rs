use super::*;
use crate::lists::Running;
use crate::lists::fixtures::{NOON, item, lists, stamp};
use crate::lists::store::Store;
use crate::testing::Scratch;
use serde_json::{Value, json};
use std::sync::mpsc;
use std::time::Duration;

fn loaded(project: &str, lists: Result<crate::lists::Lists, String>) -> Loaded {
    Loaded { project: project.to_string(), lists }
}

fn no_claims(_: &str) -> ClaimFacts {
    panic!("no item runs")
}

fn error_of(snapshot: Snapshot) -> String {
    let view = serde_json::to_value(compose(&snapshot, NOON, "quick", &no_claims)).unwrap();
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
    state.item_mut(running).unwrap().running = Some(Running {
        worktree: worktree.into(),
        branch: "import-tags".into(),
        brief: "plans/briefs/x.md".into(),
        since: stamp(NOON),
    });
    let snapshot: Snapshot = Ok(vec![loaded("demo", Ok(state))]);
    assert_eq!(running_worktrees(&snapshot), vec![worktree.to_string()]);
    let facts = ClaimFacts {
        agent: Some("import-tags".into()),
        note: Some("tests next".into()),
        note_state: Some("held".into()),
        waiting_to_land: 2,
    };
    let view = serde_json::to_value(compose(&snapshot, NOON, "quick", &|asked| {
        assert_eq!(asked, worktree);
        facts.clone()
    }))
    .unwrap();
    let find = |id: crate::lists::Id| view["items"].as_array().unwrap().iter().find(|i| i["id"] == json!(id.0)).unwrap().clone();
    let r = &find(running)["running"];
    assert_eq!(
        (&r["agent"], &r["note"], &r["note_state"], &r["waiting_to_land"]),
        (&json!("import-tags"), &json!("tests next"), &json!("held"), &json!(2))
    );
    assert_eq!(find(idle)["running"], Value::Null);
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

fn design_with_render(state: &mut crate::lists::Lists, renders: &[(&str, Option<&str>)]) -> crate::lists::Id {
    let id = crate::lists::fixtures::design(state);
    let change = state.design_mut(id).unwrap();
    for (path, render) in renders {
        change.boards.insert(
            path.to_string(),
            crate::lists::Board { link: "https://canvas.example".into(), render: render.map(str::to_string) },
        );
    }
    id
}

#[test]
fn a_render_is_found_by_design_and_board_position_and_only_for_image_types() {
    let mut state = lists();
    let id = design_with_render(
        &mut state,
        &[
            ("design/mocks/A.dc.html", Some("/r/A.PNG")),
            ("design/mocks/B.dc.html", None),
            ("design/mocks/C.dc.html", Some("/r/C.svg")),
            ("design/mocks/D.dc.html", Some("/r/D.jpeg")),
        ],
    );
    let snapshot: Snapshot = Ok(vec![loaded("demo", Ok(state))]);
    assert_eq!(design_render(&snapshot, id.0, 0), Some(("/r/A.PNG".to_string(), "image/png")));
    assert_eq!(design_render(&snapshot, id.0, 1), None, "a board with no render");
    assert_eq!(design_render(&snapshot, id.0, 2), None, "svg is no render type");
    assert_eq!(design_render(&snapshot, id.0, 3), Some(("/r/D.jpeg".to_string(), "image/jpeg")));
    assert_eq!(design_render(&snapshot, id.0, 9), None);
    assert_eq!(design_render(&snapshot, id.0 + 1, 0), None);
    assert_eq!(design_render(&Ok(vec![]), id.0, 0), None);
}
