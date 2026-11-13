use super::*;
use crate::lists::record::close;
use crate::lists::fixtures::{
    NOON, blocks_something, db, decision_answering, design, item, item_opened, question, read, set_area, set_audience, stamp,
};
use crate::lists::{Audience, How, Placement, Priority, Running, Stamp};
use rusqlite::Connection;

const DAY: i64 = 86400;

fn place(db: &Connection, question: Id, at: Position) {
    crate::lists::question::place(db, question, Some(Placement { at, why: "why".into(), by_person: true })).unwrap();
}

fn priority(db: &Connection, item: Id, level: Level) {
    crate::lists::item::set_priority(db, item, Some(Priority { level, why: "why".into() })).unwrap();
}

#[test]
fn a_wait_is_met_by_the_way_its_record_closed() {
    let db = db();
    let waits = |lists: &Lists, wait: Wait| wait_met(lists, &wait);
    let answered = question(&db);
    let settled = question(&db);
    let done = question(&db);
    let dropped = question(&db);
    let deferred = question(&db);
    let open = question(&db);
    let decision = decision_answering(&db, answered);
    close(&db, answered, How::Answered(decision), &stamp(NOON)).unwrap();
    close(&db, settled, How::Settled("rule".into()), &stamp(NOON)).unwrap();
    close(&db, done, How::Done, &stamp(NOON)).unwrap();
    close(&db, dropped, How::Dropped("no".into()), &stamp(NOON)).unwrap();
    close(&db, deferred, How::Deferred("https://x/1".into()), &stamp(NOON)).unwrap();
    for (id, met) in [(answered, true), (settled, true), (done, true), (dropped, false), (deferred, false), (open, false)] {
        assert_eq!(waits(&read(&db), Wait::Question(id)), met, "{id}");
    }
    let landed = item(&db, vec![]);
    let dropped_item = item(&db, vec![]);
    let moved = item(&db, vec![]);
    let open_item = item(&db, vec![]);
    close(&db, landed, How::Landed(vec![crate::lists::fixtures::sha('a')]), &stamp(NOON)).unwrap();
    close(&db, dropped_item, How::Dropped("no".into()), &stamp(NOON)).unwrap();
    close(&db, moved, How::MovedToStack(open), &stamp(NOON)).unwrap();
    for (id, met) in [(landed, true), (dropped_item, false), (moved, false), (open_item, false)] {
        assert_eq!(waits(&read(&db), Wait::Item(id)), met, "{id}");
    }
    let board = design(&db);
    let open_board = design(&db);
    close(&db, board, How::Landed(vec![crate::lists::fixtures::sha('a')]), &stamp(NOON)).unwrap();
    assert!(waits(&read(&db), Wait::Design(board)));
    assert!(!waits(&read(&db), Wait::Design(open_board)));
    assert!(!waits(&read(&db), Wait::Condition("c".into())));
    assert!(!waits(&read(&db), Wait::Item(Id(900))));
}

#[test]
fn an_items_state() {
    let db = db();
    let q = question(&db);
    let ready = item(&db, vec![]);
    let waiting = item(&db, vec![Wait::Question(q)]);
    let condition = item(&db, vec![Wait::Condition("quiet".into())]);
    let design_session = item(&db, vec![]);
    set_audience(&db, design_session, Audience::DesignSession);
    let running = item(&db, vec![Wait::Question(q)]);
    crate::lists::running::start(&db, &[running], &[Running {
        worktree: "/d/bae-worktrees/side-1".into(),
        branch: "b".into(),
        brief: "p".into(),
        since: stamp(NOON),
    }]).unwrap();
    let state = |id| {
        let lists = read(&db);
        item_state(&lists, lists.item(id).unwrap())
    };
    assert_eq!(state(ready), ItemState::Ready);
    assert_eq!(state(waiting), ItemState::Waiting);
    assert_eq!(state(condition), ItemState::Waiting);
    assert_eq!(state(design_session), ItemState::DesignSession);
    // A running item that also waits on an answer is still running.
    assert_eq!(state(running), ItemState::Running);
    // Once the question closes, the wait is met.
    close(&db, q, How::Done, &stamp(NOON)).unwrap();
    assert_eq!(state(waiting), ItemState::Ready);
}

#[test]
fn a_last_item_waits_while_other_coordinator_work_is_open() {
    let db = db();
    let last = item(&db, vec![]);
    priority(&db, last, Level::Last);
    let other_last = item(&db, vec![]);
    priority(&db, other_last, Level::Last);
    let state = |id| {
        let lists = read(&db);
        item_state(&lists, lists.item(id).unwrap())
    };
    // Only last items: both are ready.
    assert_eq!(state(last), ItemState::Ready);
    assert_eq!(state(other_last), ItemState::Ready);
    // The design session's open items do not hold a last item.
    let design_session = item(&db, vec![]);
    set_audience(&db, design_session, Audience::DesignSession);
    assert_eq!(state(last), ItemState::Ready);
    // Other coordinator work does, even work that is itself waiting.
    let q = question(&db);
    let other = item(&db, vec![Wait::Question(q)]);
    assert_eq!(state(last), ItemState::Waiting);
    assert_eq!(state(other), ItemState::Waiting);
    close(&db, other, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    assert_eq!(state(last), ItemState::Ready);
}

fn order(db: &Connection) -> Vec<(u32, usize)> {
    stack_order(&read(db)).iter().map(|e| (e.question.id.0, e.blocked_count)).collect()
}

fn blocked_question(db: &Connection, blocks: usize) -> Id {
    let q = question(db);
    for _ in 0..blocks {
        item(db, vec![Wait::Question(q)]);
    }
    blocks_something(db, q);
    q
}

#[test]
fn the_stack_asks_what_unblocks_most_first_and_ties_oldest_first() {
    let db = db();
    let one = blocked_question(&db, 1); // #1, item #2
    let three = blocked_question(&db, 3); // #5, items #6-8
    let one_again = blocked_question(&db, 1); // #9
    assert_eq!(order(&db), vec![(three.0, 3), (one.0, 1), (one_again.0, 1)]);
}

#[test]
fn chains_of_open_items_count_and_each_item_counts_once() {
    let db = db();
    let q = blocked_question(&db, 1); // item #2 waits on q
    let direct = Id(2);
    let a = item(&db, vec![Wait::Item(direct)]);
    let b = item(&db, vec![Wait::Item(a), Wait::Item(direct)]); // reached twice
    item(&db, vec![Wait::Item(b)]);
    let other = blocked_question(&db, 3);
    assert_eq!(order(&db), vec![(q.0, 4), (other.0, 3)]);
    let lists = read(&db);
    let entry = &stack_order(&lists)[0];
    assert_eq!(entry.blocks.iter().map(|i| i.id).collect::<Vec<_>>(), vec![direct]);
    // A closed item in the chain no longer counts.
    close(&db, Id(3), How::Landed(vec![crate::lists::fixtures::sha('a')]), &stamp(NOON)).unwrap();
    assert_eq!(order(&db)[0], (q.0, 1 + 2));
}

#[test]
fn placements_sit_outside_the_count() {
    let db = db();
    let big = blocked_question(&db, 3);
    let small = blocked_question(&db, 1);
    let first_old = question(&db);
    let first_new = question(&db);
    let last = blocked_question(&db, 5);
    place(&db, first_old, Position::First);
    place(&db, first_new, Position::First);
    place(&db, last, Position::Last);
    let ids: Vec<Id> = stack_order(&read(&db)).iter().map(|e| e.question.id).collect();
    // First: newest first, whatever they block; then by count; last at the end.
    assert_eq!(ids, vec![first_new, first_old, big, small, last]);
}

#[test]
fn dispatch_order_is_first_then_area_and_age_then_last() {
    let db = db();
    let q = question(&db);
    let beta_old = item(&db, vec![]);
    set_area(&db, beta_old, "Beta");
    let alpha_new = item(&db, vec![]);
    let alpha_old = item_opened(&db, vec![], stamp(NOON - 100));
    let first_late = item_opened(&db, vec![], stamp(NOON + 500));
    priority(&db, first_late, Level::First);
    let first_early = item_opened(&db, vec![], stamp(NOON + 400));
    priority(&db, first_early, Level::First);
    let waiting = item(&db, vec![Wait::Question(q)]);
    priority(&db, waiting, Level::First);
    let design_session = item(&db, vec![]);
    set_audience(&db, design_session, Audience::DesignSession);
    let ids: Vec<Id> = dispatch_order(&read(&db)).iter().map(|i| i.id).collect();
    assert_eq!(ids, vec![first_early, first_late, alpha_old, alpha_new, beta_old]);
    // A last item is ready only once the others are gone, and then comes out.
    let last = item(&db, vec![]);
    priority(&db, last, Level::Last);
    assert!(!dispatch_order(&read(&db)).iter().any(|i| i.id == last));
    for id in [first_early, first_late, alpha_old, alpha_new, beta_old, waiting] {
        close(&db, id, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    }
    assert_eq!(dispatch_order(&read(&db)).iter().map(|i| i.id).collect::<Vec<_>>(), vec![last]);
}

#[test]
fn flow_counts_the_last_fourteen_days_by_who_opened_and_how_closed() {
    let db = db();
    let now = NOON;
    item_opened(&db, vec![], Stamp { at: now, by: "design-session".into() });
    item(&db, vec![]); // coordinator, today
    item_opened(&db, vec![], stamp(now - 3 * DAY));
    item_opened(&db, vec![], stamp(now - 40 * DAY));
    question(&db);
    let landed = item_opened(&db, vec![], stamp(now - 2 * DAY));
    close(&db, landed, How::Landed(vec![crate::lists::fixtures::sha('a')]), &stamp(now)).unwrap();
    let dropped = item(&db, vec![]);
    close(&db, dropped, How::Dropped("x".into()), &stamp(now - DAY)).unwrap();
    let board = design(&db);
    close(&db, board, How::Landed(vec![crate::lists::fixtures::sha('a')]), &stamp(now)).unwrap();

    let days = flow(&read(&db), now);
    assert_eq!(days.len(), FLOW_DAYS);
    assert_eq!(days[0].day, crate::text::local_day(now));
    assert_eq!(days[1].day, crate::text::local_day(now - DAY));
    let opened_today = BTreeMap::from([("coordinator".to_string(), 3), ("design-session".to_string(), 1)]);
    assert_eq!(days[0].opened, opened_today);
    assert_eq!(days[0].closed, BTreeMap::from([("landed".to_string(), 1)]));
    assert_eq!(days[1].closed, BTreeMap::from([("dropped".to_string(), 1)]));
    assert_eq!(days[2].opened, BTreeMap::from([("coordinator".to_string(), 1)]));
    assert_eq!(days[3].opened, BTreeMap::from([("coordinator".to_string(), 1)]));
    // The ancient item, and the design change (not an item or a question),
    // count nowhere.
    let total: u32 = days.iter().flat_map(|d| d.opened.values()).sum();
    assert_eq!(total, 6);
}
