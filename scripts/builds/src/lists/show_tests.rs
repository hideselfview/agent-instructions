use super::*;
use crate::lists::fixtures::{NOON, blocks_something, db, item, question, read, stamp};
use crate::lists::record::close;
use crate::lists::{How, Priority};

#[test]
fn the_text_lists_the_stack_then_the_queue() {
    let db = db();
    let q = question(&db);
    crate::lists::question::place(&db, q, crate::lists::question::Spot::Last, "later", true)
        .unwrap();
    let blocked = item(
        &db,
        vec![Wait::Question(q), Wait::Condition("quiet".into())],
    );
    let first = item(&db, vec![]);
    crate::lists::item::set_priority(
        &db,
        first,
        Some(Priority {
            level: Level::First,
            why: "w".into(),
        }),
    )
    .unwrap();
    let gone = item(&db, vec![]);
    close(&db, gone, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    let text = render(&read(&db), None).unwrap();
    assert!(text.starts_with("demo lists, version 1\n"), "{text}");
    for expected in [
        "Waiting on you (1)",
        &format!("  1. {q} Which way for number {} [question]", q.0),
        "you placed it last: later",
        "blocks 1 (1 through chains):",
        &format!("  {blocked} Do the work numbered {}", blocked.0),
        "Work (1 ready, 0 running)",
        "First",
        &format!("  {first} [ready]"),
        "Alpha",
        &format!("  {blocked} [waiting]"),
        &format!("waits on question {q}"),
        "condition: quiet",
        "1 closed",
    ] {
        assert!(text.contains(expected), "missing '{expected}' in:\n{text}");
    }
    assert!(!text.contains(&format!("{gone} [")));
}

#[test]
fn a_question_shows_each_item_it_blocks_with_its_priority_what_waits_behind_it_and_when_it_opened()
{
    let db = db();
    let q = question(&db);
    let blocked = item(&db, vec![Wait::Question(q)]);
    crate::lists::item::set_priority(
        &db,
        blocked,
        Some(Priority {
            level: Level::First,
            why: "the person asked".into(),
        }),
    )
    .unwrap();
    item(&db, vec![Wait::Item(blocked)]);
    let text = render(&read(&db), None).unwrap();
    let opened = local_minute(NOON + i64::from(blocked.0));
    for expected in [
        "blocks 1 (2 through chains):".to_string(),
        format!(
            "  {blocked} Do the work numbered {} (priority first: the person asked; 1 item waits behind it; opened {opened})",
            blocked.0
        ),
    ] {
        assert!(text.contains(&expected), "missing '{expected}' in:\n{text}");
    }
    assert!(!text.contains("blocks nothing"), "{text}");
}

#[test]
fn a_question_with_no_waiting_item_and_no_reason_is_an_error() {
    let db = db();
    let q = question(&db);
    blocks_something(&db, q);
    let message = render(&read(&db), None).unwrap_err().text();
    assert!(
        message.contains(&format!(
            "question {q} blocks no open item and gives no reason"
        )),
        "{message}"
    );
}

#[test]
fn a_stack_entry_starts_with_who_asked_it_its_effect_and_what_it_blocks() {
    let db = db();
    let q = question(&db);
    let blocked = item(&db, vec![Wait::Question(q)]);
    let lists = read(&db);
    let entry = &derive::stack_order(&lists)[0];
    let text = stack_entry(&lists, 1, entry).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            format!("  1. {q} Which way for number {} [question]", q.0),
            format!(
                "       asked by coordinator, {}",
                local_minute(NOON + i64::from(q.0))
            ),
            "       effect: cleanup".to_string(),
            "       blocks 1 (1 through chains):".to_string(),
            format!(
                "         {blocked} Do the work numbered {} (priority none; no item waits behind it; opened {})",
                blocked.0,
                local_minute(NOON + i64::from(blocked.0))
            ),
        ]
    );
}

#[test]
fn a_stack_line_shows_the_effect_what_the_entry_blocks_and_who_asked_it_on_one_line() {
    let db = db();
    let q = question(&db);
    let blocked = item(&db, vec![Wait::Question(q)]);
    item(&db, vec![Wait::Item(blocked)]);
    let lists = read(&db);
    let entry = &derive::stack_order(&lists)[0];
    assert_eq!(
        stack_line(&lists, 1, entry).unwrap(),
        format!(
            "  1. {q} Which way for number {}  cleanup · blocks 2 · coordinator, {}\n",
            q.0,
            local_minute(NOON + i64::from(q.0))
        )
    );
}

#[test]
fn a_stack_line_leaves_out_why_an_entry_blocks_nothing_and_why_someone_placed_it() {
    let db = db();
    let q = question(&db);
    crate::lists::question::place(&db, q, crate::lists::question::Spot::Last, "later", true)
        .unwrap();
    let lists = read(&db);
    let entry = &derive::stack_order(&lists)[0];
    assert_eq!(
        stack_line(&lists, 1, entry).unwrap(),
        format!(
            "  1. {q} Which way for number {}  cleanup · coordinator, {}\n",
            q.0,
            local_minute(NOON + i64::from(q.0))
        )
    );
}
