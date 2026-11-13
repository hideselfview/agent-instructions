use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, item, lists, question, stamp};
use crate::lists::item::{ItemDraft, ItemEdit};
use crate::lists::record::close;

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn waiting_lists() -> (Lists, Id, Id, Id) {
    let mut lists = lists();
    let q = question(&mut lists);
    let first = lists.questions[0].id;
    crate::lists::fixtures::blocks_something(&mut lists, first);
    let a = item(&mut lists, vec![Wait::Question(q)]);
    let b = item(&mut lists, vec![Wait::Question(q)]);
    (lists, q, a, b)
}

#[test]
fn the_blocked_map_names_every_waiting_item_and_only_those() {
    let (lists, q, a, b) = waiting_lists();
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
    let (mut lists, q, a, b) = waiting_lists();
    let edit = ItemEdit { title: None, area: None, details: Some("changed".into()), brief: None, r#for: None };
    let blocked = BlockedMap::from([(a, Blocked::Edit(edit)), (b, Blocked::Drop("not needed".into()))]);
    apply_blocked(&mut lists, q, &blocked, &stamp(NOON)).unwrap();
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
    let (mut lists, q, a, b) = waiting_lists();
    let c = item(&mut lists, vec![Wait::Item(a)]);
    let blocked = BlockedMap::from([(a, Blocked::Drop("no".into())), (b, Blocked::Proceeds)]);
    apply_blocked(&mut lists, q, &blocked, &stamp(NOON)).unwrap();
    close(&mut lists, q, How::Done, &stamp(NOON)).unwrap();
    let message = refusal(crate::lists::check::check(&lists));
    assert!(message.contains(&c.to_string()) && message.contains("can no longer be met"), "{message}");
}
