use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, draft, item, lists, question, stamp};
use crate::lists::record::close;
use crate::lists::How;

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

#[test]
fn a_draft_needs_five_to_seven_words_a_known_area_and_details() {
    let lists = lists();
    assert!(refusal(draft("Too short").validate(&lists)).contains("2 words"));
    assert!(refusal(draft("One two three four five six seven eight").validate(&lists)).contains("8 words"));
    let mut elsewhere = draft("One two three four five");
    elsewhere.area = "Nowhere".into();
    let message = refusal(elsewhere.validate(&lists));
    assert!(message.contains("'Nowhere'") && message.contains("Alpha; Beta") && message.contains("area add"), "{message}");
    let mut empty = draft("One two three four five");
    empty.details = " ".into();
    assert!(refusal(empty.validate(&lists)).contains("details"));
    assert!(draft("One two three four five").validate(&lists).is_ok());
}

#[test]
fn a_draft_waits_only_on_open_records() {
    let mut lists = lists();
    let q = question(&mut lists);
    let open = item(&mut lists, vec![]);
    let gone = item(&mut lists, vec![]);
    close(&mut lists, gone, How::Landed(vec!["a".into()]), &stamp(NOON)).unwrap();
    let with = |wait: Wait| {
        let mut d = draft("One two three four five");
        d.waits = vec![wait];
        d.validate(&lists)
    };
    assert!(with(Wait::Question(q)).is_ok());
    assert!(with(Wait::Item(open)).is_ok());
    assert!(with(Wait::Condition("quiet".into())).is_ok());
    assert!(refusal(with(Wait::Item(gone))).contains("is closed (landed)"));
    assert!(refusal(with(Wait::Item(Id(99)))).contains("no record has"));
    assert!(refusal(with(Wait::Item(q))).contains("as a item, but it is a question"));
    assert!(refusal(with(Wait::Condition(" ".into()))).contains("condition"));
}

#[test]
fn add_item_makes_a_stamped_item_with_the_next_id() {
    let mut lists = lists();
    let mut d = draft("One two three four five");
    d.priority = Some(Priority { level: Level::First, why: "why".into() });
    let id = add_item(&mut lists, &d, Some(Id(1)), &stamp(NOON)).unwrap();
    let made = lists.item(id).unwrap();
    assert_eq!((made.id, made.from, made.opened.clone()), (Id(1), Some(Id(1)), stamp(NOON)));
    assert_eq!(made.priority, d.priority);
    assert_eq!(lists.next_id, Id(2));
    // A refused draft hands out no id.
    let before = lists.clone();
    assert!(add_item(&mut lists, &draft("Short"), None, &stamp(NOON)).is_err());
    assert_eq!(lists, before);
}

#[test]
fn an_edit_changes_named_fields_by_the_drafts_rules() {
    let mut lists = lists();
    let id = item(&mut lists, vec![]);
    let edit = |f: &dyn Fn(&mut ItemEdit)| {
        let mut e = ItemEdit { title: None, area: None, details: None, brief: None, r#for: None };
        f(&mut e);
        e
    };
    assert!(refusal(edit(&|_| {}).apply(&mut lists, id)).contains("names no field"));
    assert!(refusal(edit(&|e| e.title = Some("Short".into())).apply(&mut lists, id)).contains("words"));
    assert!(refusal(edit(&|e| e.area = Some("Nowhere".into())).apply(&mut lists, id)).contains("Nowhere"));
    assert!(refusal(edit(&|e| e.details = Some("".into())).apply(&mut lists, id)).contains("details"));
    assert!(refusal(edit(&|e| e.details = Some("d".into())).apply(&mut lists, Id(99))).contains("not an open item"));
    edit(&|e| {
        e.title = Some("A new title of five".into());
        e.area = Some("Beta".into());
        e.details = Some("new".into());
        e.brief = Some("b.md".into());
        e.r#for = Some(Audience::DesignSession);
    })
    .apply(&mut lists, id)
    .unwrap();
    let it = lists.item(id).unwrap();
    assert_eq!(
        (it.title.as_str(), it.area.as_str(), it.details.as_str(), it.brief.as_deref(), it.r#for),
        ("A new title of five", "Beta", "new", Some("b.md"), Audience::DesignSession)
    );
}
