use super::*;
use crate::fail::Fail;
use crate::lists::Kind;
use crate::lists::fixtures::{NOON, blocks_something, db, item, question, read, stamp};
use crate::lists::item::{ItemDraft, ItemEdit};
use crate::lists::record::close;
use rusqlite::Connection;

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn waiting_lists() -> (Connection, Id, Id, Id) {
    let db = db();
    let q = question(&db);
    let a = item(&db, vec![Wait::Question(q)]);
    let b = item(&db, vec![Wait::Question(q)]);
    blocks_something(&db, q);
    (db, q, a, b)
}

#[test]
fn the_blocked_map_names_every_waiting_item_and_only_those() {
    let (db, q, a, b) = waiting_lists();
    let lists = read(&db);
    let other = Id(99);
    let both = BlockedMap::from([(a, Blocked::Proceeds), (b, Blocked::Drop("no".into()))]);
    assert!(check_blocked(&lists, q, &both).is_ok());
    let left_out = BlockedMap::from([(a, Blocked::Proceeds)]);
    let message = refusal(check_blocked(&lists, q, &left_out));
    assert!(message.contains(&b.to_string()) && message.contains("proceeds"), "{message}");
    let stray = BlockedMap::from([(a, Blocked::Proceeds), (b, Blocked::Proceeds), (other, Blocked::Proceeds)]);
    assert!(refusal(check_blocked(&lists, q, &stray)).contains("#99"));
    let empty_drop = BlockedMap::from([(a, Blocked::Proceeds), (b, Blocked::Drop(" ".into()))]);
    assert!(refusal(check_blocked(&lists, q, &empty_drop)).contains("no reason"));
    let no_field = ItemEdit { title: None, area: None, details: None, brief: None, r#for: None };
    let empty_edit = BlockedMap::from([(a, Blocked::Proceeds), (b, Blocked::Edit(no_field))]);
    assert!(refusal(check_blocked(&lists, q, &empty_edit)).contains("no field"));
}

#[test]
fn applying_blocked_edits_and_drops() {
    let (db, q, a, b) = waiting_lists();
    let edit = ItemEdit { title: None, area: None, details: Some("changed".into()), brief: None, r#for: None };
    let blocked = BlockedMap::from([(a, Blocked::Edit(edit)), (b, Blocked::Drop("not needed".into()))]);
    apply_blocked(&db, q, &blocked, &stamp(NOON)).unwrap();
    let lists = read(&db);
    assert_eq!(lists.item(a).unwrap().details, "changed");
    assert_eq!(lists.closed_record(b).unwrap().how, How::Dropped("not needed".into()));
}

#[test]
fn blocked_reads_from_json() {
    let blocked: BlockedMap =
        serde_json::from_str(r#"{"12":"proceeds","13":{"edit":{"details":"d"}},"14":{"drop":"why"}}"#).unwrap();
    assert_eq!(blocked.len(), 3);
    assert_eq!(blocked[&Id(14)], Blocked::Drop("why".into()));
    assert!(serde_json::from_str::<BlockedMap>(r#"{"12":{"edit":{"colour":"x"}}}"#).is_err());
    let draft: ItemDraft = serde_json::from_str(
        r#"{"title":"A b c d e","area":"Alpha","details":"d","for":"coordinator",
            "waits":[{"item":412},{"question":3},{"condition":"a quiet machine"}],
            "brief":"plans/briefs/import.md#Filtered list","priority":null}"#,
    )
    .unwrap();
    assert_eq!(draft.waits.len(), 3);
    assert!(serde_json::from_str::<ItemDraft>(r#"{"title":"x"}"#).is_err());
}

#[test]
fn a_dropped_item_others_wait_on_fails_the_final_check() {
    let (db, q, a, b) = waiting_lists();
    let c = item(&db, vec![Wait::Item(a)]);
    let blocked = BlockedMap::from([(a, Blocked::Drop("no".into())), (b, Blocked::Proceeds)]);
    apply_blocked(&db, q, &blocked, &stamp(NOON)).unwrap();
    close(&db, q, How::Done, &stamp(NOON)).unwrap();
    let faults = crate::lists::check::faults(&db, "demo").unwrap();
    let message = faults.join("\n");
    assert!(message.contains(&c.to_string()) && message.contains("can no longer be met"), "{message}");
}

#[test]
fn every_named_constraint_of_the_question_tables_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, row, v};
    use crate::lists::record::open;
    let db = db();
    let good = open(&db, Kind::Question, &stamp(1)).unwrap();
    let spare = open(&db, Kind::Question, &stamp(1)).unwrap();
    let question = [("id", v(good.0)), ("title", v("A title")), ("details", v("d")), ("kind", v("question"))];
    plant(&db, "question", &question).unwrap();
    let mut refused = Refusals::new(include_str!("question.sql"));
    let spare_row = |over: &[(&'static str, rusqlite::types::Value)]| row(&row(&question, &[("id", v(spare.0))]), over);
    refused.check("question_is_a_question", plant(&db, "question", &spare_row(&[("record_kind", v("item"))])));
    for title in ["", " x", "a\nb", "a\rb", "a\tb"] {
        refused.check("question_title_is_one_line", plant(&db, "question", &spare_row(&[("title", v(title))])));
    }
    refused.check("question_details_say_something", plant(&db, "question", &spare_row(&[("details", v(" "))])));
    refused.check("question_kind_known", plant(&db, "question", &spare_row(&[("kind", v("riddle"))])));
    refused.check("question_blocks_nothing_says_something", plant(&db, "question", &spare_row(&[("blocks_nothing", v(" "))])));
    refused.check_as("question_is_a_record", "FOREIGN KEY", plant(&db, "question", &row(&question, &[("id", v(900))])));
    let placement = [("question_id", v(good.0)), ("at", v("first")), ("why", v("w")), ("by_person", v(1))];
    plant(&db, "question_placement", &placement).unwrap();
    let again = |over: &[(&'static str, rusqlite::types::Value)]| row(&row(&placement, &[("question_id", v(good.0))]), over);
    let _ = again;
    db.execute("DELETE FROM question_placement", []).unwrap();
    refused.check("question_placement_at_known", plant(&db, "question_placement", &row(&placement, &[("at", v("middle"))])));
    refused.check("question_placement_why_says_something", plant(&db, "question_placement", &row(&placement, &[("why", v(" "))])));
    refused.check("question_placement_by_person_is_a_flag", plant(&db, "question_placement", &row(&placement, &[("by_person", v(2))])));
    let plan = [("question_id", v(good.0)), ("path", v("plans/p.md")), ("number", v("Q1"))];
    refused.check("question_plan_path_says_something", plant(&db, "question_plan", &row(&plan, &[("path", v(" "))])));
    refused.check("question_plan_number_says_something", plant(&db, "question_plan", &row(&plan, &[("number", v(""))])));
    plant(&db, "question_plan", &plan).unwrap();
    refused.done();
}
