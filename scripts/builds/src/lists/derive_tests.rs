use super::*;
use crate::lists::draft::close;
use crate::lists::fixtures::{NOON, design, item, lists, question, stamp};
use crate::lists::{Audience, How, Placement, Priority, Running, Stamp};

const DAY: i64 = 86400;

fn place(lists: &mut Lists, question: Id, at: Position) {
    lists.questions.iter_mut().find(|q| q.id == question).unwrap().placed =
        Some(Placement { at, why: "why".into(), by_person: true });
}

fn priority(lists: &mut Lists, item: Id, level: Level) {
    lists.item_mut(item).unwrap().priority = Some(Priority { level, why: "why".into() });
}

#[test]
fn a_wait_is_met_by_the_way_its_record_closed() {
    let mut lists = lists();
    let waits = |lists: &Lists, wait: Wait| wait_met(lists, &wait);
    let answered = question(&mut lists);
    let settled = question(&mut lists);
    let done = question(&mut lists);
    let dropped = question(&mut lists);
    let deferred = question(&mut lists);
    let open = question(&mut lists);
    let decision = Id(500);
    close(&mut lists, answered, How::Answered(decision), &stamp(NOON)).unwrap();
    close(&mut lists, settled, How::Settled("rule".into()), &stamp(NOON)).unwrap();
    close(&mut lists, done, How::Done, &stamp(NOON)).unwrap();
    close(&mut lists, dropped, How::Dropped("no".into()), &stamp(NOON)).unwrap();
    close(&mut lists, deferred, How::Deferred("https://x/1".into()), &stamp(NOON)).unwrap();
    for (id, met) in [(answered, true), (settled, true), (done, true), (dropped, false), (deferred, false), (open, false)] {
        assert_eq!(waits(&lists, Wait::Question(id)), met, "{id}");
    }
    let landed = item(&mut lists, vec![]);
    let dropped_item = item(&mut lists, vec![]);
    let moved = item(&mut lists, vec![]);
    let open_item = item(&mut lists, vec![]);
    close(&mut lists, landed, How::Landed(vec!["a".into()]), &stamp(NOON)).unwrap();
    close(&mut lists, dropped_item, How::Dropped("no".into()), &stamp(NOON)).unwrap();
    close(&mut lists, moved, How::MovedToStack(open), &stamp(NOON)).unwrap();
    for (id, met) in [(landed, true), (dropped_item, false), (moved, false), (open_item, false)] {
        assert_eq!(waits(&lists, Wait::Item(id)), met, "{id}");
    }
    let board = design(&mut lists);
    let open_board = design(&mut lists);
    close(&mut lists, board, How::Landed(vec!["a".into()]), &stamp(NOON)).unwrap();
    assert!(waits(&lists, Wait::Design(board)));
    assert!(!waits(&lists, Wait::Design(open_board)));
    assert!(!waits(&lists, Wait::Condition("c".into())));
    assert!(!waits(&lists, Wait::Item(Id(900))));
}

#[test]
fn an_items_state() {
    let mut lists = lists();
    let q = question(&mut lists);
    let ready = item(&mut lists, vec![]);
    let waiting = item(&mut lists, vec![Wait::Question(q)]);
    let condition = item(&mut lists, vec![Wait::Condition("quiet".into())]);
    let design_session = item(&mut lists, vec![]);
    lists.item_mut(design_session).unwrap().r#for = Audience::DesignSession;
    let running = item(&mut lists, vec![Wait::Question(q)]);
    lists.item_mut(running).unwrap().running = vec![Running {
        worktree: "/d/bae-worktrees/side-1".into(),
        branch: "b".into(),
        brief: "p".into(),
        since: stamp(NOON),
    }];
    let state = |id| item_state(&lists, lists.item(id).unwrap());
    assert_eq!(state(ready), ItemState::Ready);
    assert_eq!(state(waiting), ItemState::Waiting);
    assert_eq!(state(condition), ItemState::Waiting);
    assert_eq!(state(design_session), ItemState::DesignSession);
    // A running item that also waits on an answer is still running.
    assert_eq!(state(running), ItemState::Running);
    // Once the question closes, the wait is met.
    close(&mut lists, q, How::Done, &stamp(NOON)).unwrap();
    assert_eq!(item_state(&lists, lists.item(waiting).unwrap()), ItemState::Ready);
}

#[test]
fn a_last_item_waits_while_other_coordinator_work_is_open() {
    let mut lists = lists();
    let last = item(&mut lists, vec![]);
    priority(&mut lists, last, Level::Last);
    let other_last = item(&mut lists, vec![]);
    priority(&mut lists, other_last, Level::Last);
    let state = |lists: &Lists, id| item_state(lists, lists.item(id).unwrap());
    // Only last items: both are ready.
    assert_eq!(state(&lists, last), ItemState::Ready);
    assert_eq!(state(&lists, other_last), ItemState::Ready);
    // The design session's open items do not hold a last item.
    let design_session = item(&mut lists, vec![]);
    lists.item_mut(design_session).unwrap().r#for = Audience::DesignSession;
    assert_eq!(state(&lists, last), ItemState::Ready);
    // Other coordinator work does, even work that is itself waiting.
    let q = question(&mut lists);
    let other = item(&mut lists, vec![Wait::Question(q)]);
    assert_eq!(state(&lists, last), ItemState::Waiting);
    assert_eq!(state(&lists, other), ItemState::Waiting);
    close(&mut lists, other, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    assert_eq!(state(&lists, last), ItemState::Ready);
}

fn order(lists: &Lists) -> Vec<(u32, usize)> {
    stack_order(lists).iter().map(|e| (e.question.id.0, e.blocked_count)).collect()
}

fn blocked_question(lists: &mut Lists, blocks: usize) -> Id {
    let q = question(lists);
    lists.questions.last_mut().unwrap().blocks_nothing = None;
    for _ in 0..blocks {
        item(lists, vec![Wait::Question(q)]);
    }
    q
}

#[test]
fn the_stack_asks_what_unblocks_most_first_and_ties_oldest_first() {
    let mut lists = lists();
    let one = blocked_question(&mut lists, 1); // #1, item #2
    let three = blocked_question(&mut lists, 3); // #5, items #6-8
    let one_again = blocked_question(&mut lists, 1); // #9
    assert_eq!(order(&lists), vec![(three.0, 3), (one.0, 1), (one_again.0, 1)]);
}

#[test]
fn chains_of_open_items_count_and_each_item_counts_once() {
    let mut lists = lists();
    let q = blocked_question(&mut lists, 1); // item #2 waits on q
    let direct = Id(2);
    let a = item(&mut lists, vec![Wait::Item(direct)]);
    let b = item(&mut lists, vec![Wait::Item(a), Wait::Item(direct)]); // reached twice
    item(&mut lists, vec![Wait::Item(b)]);
    let other = blocked_question(&mut lists, 3);
    assert_eq!(order(&lists), vec![(q.0, 4), (other.0, 3)]);
    let entry = &stack_order(&lists)[0];
    assert_eq!(entry.blocks.iter().map(|i| i.id).collect::<Vec<_>>(), vec![direct]);
    // A closed item in the chain no longer counts.
    close(&mut lists, Id(3), How::Landed(vec!["x".into()]), &stamp(NOON)).unwrap();
    assert_eq!(order(&lists)[0], (q.0, 1 + 2));
}

#[test]
fn placements_sit_outside_the_count() {
    let mut lists = lists();
    let big = blocked_question(&mut lists, 3);
    let small = blocked_question(&mut lists, 1);
    let first_old = question(&mut lists);
    let first_new = question(&mut lists);
    let last = blocked_question(&mut lists, 5);
    place(&mut lists, first_old, Position::First);
    place(&mut lists, first_new, Position::First);
    place(&mut lists, last, Position::Last);
    let ids: Vec<Id> = stack_order(&lists).iter().map(|e| e.question.id).collect();
    // First: newest first, whatever they block; then by count; last at the end.
    assert_eq!(ids, vec![first_new, first_old, big, small, last]);
}

#[test]
fn dispatch_order_is_first_then_area_and_age_then_last() {
    let mut lists = lists();
    let q = question(&mut lists);
    let beta_old = item(&mut lists, vec![]);
    lists.item_mut(beta_old).unwrap().area = "Beta".into();
    let alpha_new = item(&mut lists, vec![]);
    let alpha_old = item(&mut lists, vec![]);
    lists.item_mut(alpha_old).unwrap().opened.at = NOON - 100;
    let first_late = item(&mut lists, vec![]);
    priority(&mut lists, first_late, Level::First);
    lists.item_mut(first_late).unwrap().opened.at = NOON + 500;
    let first_early = item(&mut lists, vec![]);
    priority(&mut lists, first_early, Level::First);
    lists.item_mut(first_early).unwrap().opened.at = NOON + 400;
    let waiting = item(&mut lists, vec![Wait::Question(q)]);
    priority(&mut lists, waiting, Level::First);
    let design_session = item(&mut lists, vec![]);
    lists.item_mut(design_session).unwrap().r#for = Audience::DesignSession;
    let ids: Vec<Id> = dispatch_order(&lists).iter().map(|i| i.id).collect();
    assert_eq!(ids, vec![first_early, first_late, alpha_old, alpha_new, beta_old]);
    // A last item is ready only once the others are gone, and then comes out.
    let last = item(&mut lists, vec![]);
    priority(&mut lists, last, Level::Last);
    assert!(!dispatch_order(&lists).iter().any(|i| i.id == last));
    for id in [first_early, first_late, alpha_old, alpha_new, beta_old, waiting] {
        close(&mut lists, id, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    }
    assert_eq!(dispatch_order(&lists).iter().map(|i| i.id).collect::<Vec<_>>(), vec![last]);
}

#[test]
fn flow_counts_the_last_fourteen_days_by_who_opened_and_how_closed() {
    let mut lists = lists();
    let now = NOON;
    let today_item = item(&mut lists, vec![]);
    lists.item_mut(today_item).unwrap().opened = Stamp { at: now, by: "design-session".into() };
    item(&mut lists, vec![]); // coordinator, today
    let older = item(&mut lists, vec![]);
    lists.item_mut(older).unwrap().opened.at = now - 3 * DAY;
    let ancient = item(&mut lists, vec![]);
    lists.item_mut(ancient).unwrap().opened.at = now - 40 * DAY;
    question(&mut lists);
    let landed = item(&mut lists, vec![]);
    lists.item_mut(landed).unwrap().opened.at = now - 2 * DAY;
    close(&mut lists, landed, How::Landed(vec!["a".into()]), &stamp(now)).unwrap();
    let dropped = item(&mut lists, vec![]);
    close(&mut lists, dropped, How::Dropped("x".into()), &stamp(now - DAY)).unwrap();
    let board = design(&mut lists);
    close(&mut lists, board, How::Landed(vec!["a".into()]), &stamp(now)).unwrap();

    let days = flow(&lists, now);
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
