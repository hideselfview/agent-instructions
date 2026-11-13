use super::*;
use crate::lists::draft::{NewDecision, add_decision, close};
use crate::lists::fixtures::{NOON, design, draft, item, lists, question, stamp};
use crate::lists::{How, Resolution, Running, Wait};
use serde_json::{Value, json};

fn view_json(lists: &Lists) -> Value {
    serde_json::to_value(View::build(lists, NOON)).unwrap()
}

#[test]
fn the_json_has_the_pages_shape() {
    let mut lists = lists();
    let q = question(&mut lists);
    lists.questions[0].blocks_nothing = None;
    let waiting = item(&mut lists, vec![Wait::Question(q), Wait::Condition("a quiet machine".into())]);
    let running = item(&mut lists, vec![]);
    lists.item_mut(running).unwrap().running = Some(Running {
        worktree: "/d/bae-worktrees/side-2".into(),
        branch: "import-tags".into(),
        brief: "plans/briefs/x.md".into(),
        since: stamp(NOON),
    });
    let board = design(&mut lists);
    lists.design_mut(board).unwrap().places.insert("design/mocks/A.dc.html".into(), Resolution::Item(waiting));
    lists.design_mut(board).unwrap().places.insert("design/system/components/B/".into(), Resolution::Matches("diffed".into()));
    let new = NewDecision { title: "Chose".into(), by: "person".into(), body: "why".into(), question: None, replaces: None, plan: None };
    let first = add_decision(&mut lists, &new, &[draft("One two three four five")], &stamp(NOON)).unwrap();
    let second = add_decision(&mut lists, &NewDecision { replaces: Some(first.decision), ..new }, &[], &stamp(NOON)).unwrap();

    let json = view_json(&lists);
    assert_eq!((&json["project"], &json["version"], &json["error"]), (&json!("demo"), &json!(1), &Value::Null));
    let stack = &json["stack"][0];
    assert_eq!((&stack["id"], &stack["kind"], &stack["blocked_count"]), (&json!(q.0), &json!("question"), &json!(1)));
    assert_eq!(stack["blocks"], json!([{"id": waiting.0, "title": lists.item(waiting).unwrap().title}]));
    assert_eq!(stack["blocks_nothing"], Value::Null);
    let item_json = json["items"].as_array().unwrap().iter().find(|i| i["id"] == json!(waiting.0)).unwrap();
    assert_eq!((&item_json["state"], &item_json["for"]), (&json!("waiting"), &json!("coordinator")));
    assert_eq!(item_json["waits"][0], json!({"kind": "question", "id": q.0, "title": lists.question(q).unwrap().title, "text": null, "met": false}));
    assert_eq!(item_json["waits"][1], json!({"kind": "condition", "id": null, "title": null, "text": "a quiet machine", "met": false}));
    let running_json = json["items"].as_array().unwrap().iter().find(|i| i["id"] == json!(running.0)).unwrap();
    assert_eq!(running_json["state"], json!("running"));
    let r = &running_json["running"];
    assert_eq!((&r["repo"], &r["checkout"], &r["branch"]), (&json!("bae"), &json!("side-2"), &json!("import-tags")));
    let design_json = &json["designs"][0];
    assert_eq!(design_json["closed"], Value::Null);
    let places = design_json["places"].as_array().unwrap();
    let item_place = places.iter().find(|p| p["path"] == json!("design/mocks/A.dc.html")).unwrap();
    assert_eq!(item_place["item"]["state"], json!("waiting"));
    let matches_place = places.iter().find(|p| p["matches"] == json!("diffed")).unwrap();
    assert_eq!(matches_place["item"], Value::Null);
    assert_eq!((&design_json["approved"], &design_json["blocks"]), (&Value::Null, &json!([])));
    // Decisions: newest first (same day: higher id first), linked both ways.
    assert_eq!(json["decisions"][0]["id"], json!(second.decision.0));
    assert_eq!(json["decisions"][0]["replaces"], json!(first.decision.0));
    assert_eq!(json["decisions"][1]["replaced_by"], json!(second.decision.0));
    assert_eq!(json["decisions"][1]["work"][0]["open"], json!(true));
    assert_eq!(json["flow"].as_array().unwrap().len(), 14);
}

#[test]
fn designs_that_landed_recently_show_closed_and_old_ones_drop_off() {
    let mut lists = lists();
    let recent = design(&mut lists);
    let old = design(&mut lists);
    close(&mut lists, recent, How::Landed(vec!["a".into()]), &stamp(NOON - 86400)).unwrap();
    close(&mut lists, old, How::Landed(vec!["b".into()]), &stamp(NOON - 20 * 86400)).unwrap();
    let json = view_json(&lists);
    let designs = json["designs"].as_array().unwrap();
    assert_eq!(designs.len(), 1);
    assert_eq!(designs[0]["id"], json!(recent.0));
    assert_eq!(designs[0]["closed"]["at"], json!(NOON - 86400));
}

#[test]
fn a_closed_item_a_place_names_reads_closed() {
    let mut lists = lists();
    let board = design(&mut lists);
    let work = item(&mut lists, vec![]);
    lists.design_mut(board).unwrap().places.insert("p".into(), Resolution::Item(work));
    close(&mut lists, work, How::Landed(vec!["a".into()]), &stamp(NOON)).unwrap();
    assert_eq!(view_json(&lists)["designs"][0]["places"][0]["item"]["state"], json!("closed"));
}

#[test]
fn a_failed_store_shows_its_error_and_nothing_else() {
    let json = serde_json::to_value(View::failed("demo", 7, "it is broken".into())).unwrap();
    assert_eq!((&json["error"], &json["version"], &json["stack"]), (&json!("it is broken"), &json!(7), &json!([])));
}

#[test]
fn a_design_change_shows_the_items_waiting_on_it_and_its_approval() {
    let mut lists = lists();
    let id = design(&mut lists);
    let waiting = item(&mut lists, vec![Wait::Design(id)]);
    let free = item(&mut lists, vec![]);
    let _ = free;
    lists.design_mut(id).unwrap().approved = Some(stamp(NOON));
    let json = view_json(&lists);
    assert_eq!(json["designs"][0]["blocks"], json!([{"id": waiting.0, "title": lists.item(waiting).unwrap().title}]));
    assert_eq!(json["designs"][0]["approved"]["at"], json!(NOON));
    assert_eq!(json["designs"][0].get("boards"), None, "no render, no image of a board, in the lists");
}
