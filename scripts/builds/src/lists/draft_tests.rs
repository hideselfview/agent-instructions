use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, draft, item, lists, question, stamp};
use crate::lists::Level;

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
fn a_brief_must_be_a_file() {
    let scratch = crate::testing::Scratch::new("brief");
    std::fs::create_dir_all(scratch.dir.join("plans/briefs")).unwrap();
    std::fs::write(scratch.dir.join("plans/briefs/x.md"), "# Section\n").unwrap();
    assert!(require_brief(&scratch.dir, "plans/briefs/x.md").is_ok());
    assert!(require_brief(&scratch.dir, "plans/briefs/x.md#Section").is_ok());
    assert!(refusal(require_brief(&scratch.dir, "plans/briefs/y.md")).contains("does not exist"));
    assert!(refusal(require_brief(&scratch.dir, "#Section")).contains("no file"));
    let mut d = draft("One two three four five");
    assert!(d.require_brief(&scratch.dir).is_ok());
    d.brief = Some("missing.md".into());
    assert!(d.require_brief(&scratch.dir).is_err());
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

#[test]
fn close_moves_the_record_and_refuses_what_does_not_fit() {
    let mut lists = lists();
    let q = question(&mut lists);
    let i = item(&mut lists, vec![]);
    assert!(refusal(close(&mut lists, i, How::Done, &stamp(NOON))).contains("item cannot be closed as done"));
    assert!(refusal(close(&mut lists, Id(99), How::Done, &stamp(NOON))).contains("not a record"));
    close(&mut lists, q, How::Done, &stamp(NOON + 5)).unwrap();
    assert!(lists.question(q).is_none());
    let closed = lists.closed_record(q).unwrap();
    assert_eq!((closed.how.clone(), closed.closed.clone()), (How::Done, stamp(NOON + 5)));
    assert!(refusal(close(&mut lists, q, How::Done, &stamp(NOON))).contains("already closed (done)"));
}

fn new_decision(title: &str) -> NewDecision {
    NewDecision { title: title.into(), by: "person".into(), body: "why".into(), question: None, replaces: None, plan: None }
}

#[test]
fn a_decision_records_its_items_and_dates_itself() {
    let mut lists = lists();
    let q = question(&mut lists);
    let mut new = new_decision("Chose the first way");
    new.question = Some(q);
    let work = [draft("One two three four five"), draft("Six seven eight nine ten")];
    let added = add_decision(&mut lists, &new, &work, &stamp(NOON)).unwrap();
    let decision = lists.decision(added.decision).unwrap();
    assert_eq!(decision.work, added.items);
    assert_eq!(decision.date, crate::text::local_day(NOON));
    assert_eq!(decision.question, Some(q));
    for id in &added.items {
        assert_eq!(lists.item(*id).unwrap().from, Some(added.decision));
    }
}

#[test]
fn a_decision_refused_up_front_changes_nothing() {
    let mut lists = lists();
    let before = lists.clone();
    for bad in [
        NewDecision { title: "two\nlines".into(), ..new_decision("x") },
        NewDecision { by: "".into(), ..new_decision("x") },
        NewDecision { body: " ".into(), ..new_decision("x") },
        NewDecision { question: Some(Id(9)), ..new_decision("x") },
        NewDecision { replaces: Some(Id(9)), ..new_decision("x") },
    ] {
        assert!(add_decision(&mut lists, &bad, &[], &stamp(NOON)).is_err());
        assert_eq!(lists, before);
    }
}

#[test]
fn a_decision_may_replace_an_earlier_one_and_answer_only_a_question() {
    let mut lists = lists();
    let first = add_decision(&mut lists, &new_decision("First"), &[], &stamp(NOON)).unwrap().decision;
    let mut second = new_decision("Second");
    second.replaces = Some(first);
    assert!(add_decision(&mut lists, &second, &[], &stamp(NOON)).is_ok());
    let i = item(&mut lists, vec![]);
    let mut wrong = new_decision("Third");
    wrong.question = Some(i);
    assert!(refusal(add_decision(&mut lists, &wrong, &[], &stamp(NOON))).contains("not a question"));
}

#[test]
fn work_or_no_work_is_one_of_them() {
    let one = vec![draft("One two three four five")];
    assert!(refusal(work_or_why(Some(one.clone()), Some("x".into()))).contains("not both"));
    assert!(refusal(work_or_why(None, None)).contains("`work`"));
    assert!(refusal(work_or_why(None, Some(" ".into()))).contains("reason"));
    assert_eq!(work_or_why(None, Some("nothing follows".into())).unwrap(), vec![]);
    assert_eq!(work_or_why(Some(one.clone()), None).unwrap(), one);
}

fn waiting_lists() -> (Lists, Id, Id, Id) {
    let mut lists = lists();
    let q = question(&mut lists);
    lists.questions[0].blocks_nothing = None;
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
