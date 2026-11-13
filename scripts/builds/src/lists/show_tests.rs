use super::*;
use crate::lists::record::close;
use crate::lists::fixtures::{NOON, blocks_something, db, item, question, read, stamp};
use crate::lists::{How, Placement, Priority};

#[test]
fn the_text_lists_the_stack_then_the_queue() {
    let db = db();
    let q = question(&db);
    crate::lists::question::place(&db, q, Some(Placement { at: Position::Last, why: "later".into(), by_person: true })).unwrap();
    let blocked = item(&db, vec![Wait::Question(q), Wait::Condition("quiet".into())]);
    blocks_something(&db, q);
    let first = item(&db, vec![]);
    crate::lists::item::set_priority(&db, first, Some(Priority { level: Level::First, why: "w".into() })).unwrap();
    let gone = item(&db, vec![]);
    close(&db, gone, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    let text = render(&read(&db));
    assert!(text.starts_with("demo lists, version 1\n"), "{text}");
    for expected in [
        "Waiting on you (1)",
        &format!("  1. {q} Which way for number {} [question]", q.0),
        "placed last by you: later",
        &format!("blocks 1 (1 through chains): {blocked}"),
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
