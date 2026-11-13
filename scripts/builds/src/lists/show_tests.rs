use super::*;
use crate::lists::draft::close;
use crate::lists::fixtures::{NOON, item, lists, question, stamp};
use crate::lists::{How, Placement, Priority};

#[test]
fn the_text_lists_the_stack_then_the_queue() {
    let mut lists = lists();
    let q = question(&mut lists);
    lists.questions[0].blocks_nothing = None;
    lists.questions[0].placed = Some(Placement { at: Position::Last, why: "later".into(), by_person: true });
    let blocked = item(&mut lists, vec![Wait::Question(q), Wait::Condition("quiet".into())]);
    let first = item(&mut lists, vec![]);
    lists.item_mut(first).unwrap().priority = Some(Priority { level: Level::First, why: "w".into() });
    let gone = item(&mut lists, vec![]);
    close(&mut lists, gone, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    let text = render(&lists, "quick");
    assert!(text.starts_with("demo lists, version 1, quick mode\n"), "{text}");
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
