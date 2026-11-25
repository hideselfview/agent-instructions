use super::*;
use crate::lists::fixtures::{
    NOON, db, decision_answering, design, item, item_opened, question, question_effect, read,
    set_area, set_audience, stamp,
};
use crate::lists::question::Spot;
use crate::lists::record::close;
use crate::lists::{Audience, Effect, How, Priority, Running};
use rusqlite::Connection;

fn place(db: &Connection, question: Id, spot: Spot) {
    crate::lists::question::place(db, question, spot, "why", true).unwrap();
}

fn priority(db: &Connection, item: Id, level: Level) {
    crate::lists::item::set_priority(
        db,
        item,
        Some(Priority {
            level,
            why: "why".into(),
        }),
    )
    .unwrap();
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
    close(&db, done, How::Done("shown".into()), &stamp(NOON)).unwrap();
    close(&db, dropped, How::Dropped("no".into()), &stamp(NOON)).unwrap();
    close(
        &db,
        deferred,
        How::Deferred("https://x/1".into()),
        &stamp(NOON),
    )
    .unwrap();
    for (id, met) in [
        (answered, true),
        (settled, true),
        (done, true),
        (dropped, false),
        (deferred, false),
        (open, false),
    ] {
        assert_eq!(waits(&read(&db), Wait::Question(id)), met, "{id}");
    }
    let landed = item(&db, vec![]);
    let dropped_item = item(&db, vec![]);
    let moved = item(&db, vec![]);
    let open_item = item(&db, vec![]);
    close(
        &db,
        landed,
        How::Landed(vec![crate::lists::fixtures::sha('a')]),
        &stamp(NOON),
    )
    .unwrap();
    close(&db, dropped_item, How::Dropped("no".into()), &stamp(NOON)).unwrap();
    close(&db, moved, How::MovedToStack(open), &stamp(NOON)).unwrap();
    for (id, met) in [
        (landed, true),
        (dropped_item, false),
        (moved, false),
        (open_item, false),
    ] {
        assert_eq!(waits(&read(&db), Wait::Item(id)), met, "{id}");
    }
    let board = design(&db);
    let open_board = design(&db);
    close(
        &db,
        board,
        How::Landed(vec![crate::lists::fixtures::sha('a')]),
        &stamp(NOON),
    )
    .unwrap();
    assert!(waits(&read(&db), Wait::Design(board)));
    assert!(!waits(&read(&db), Wait::Design(open_board)));
    assert!(!waits(
        &read(&db),
        Wait::condition("c", crate::lists::Ender::Coordinator)
    ));
    assert!(!waits(&read(&db), Wait::Item(Id(900))));
}

#[test]
fn an_items_state() {
    let db = db();
    let q = question(&db);
    let ready = item(&db, vec![]);
    let waiting = item(&db, vec![Wait::Question(q)]);
    let condition = item(
        &db,
        vec![Wait::condition("quiet", crate::lists::Ender::Coordinator)],
    );
    let design_session = item(&db, vec![]);
    set_audience(&db, design_session, Audience::DesignSession);
    let running = item(&db, vec![Wait::Question(q)]);
    crate::lists::running::start(
        &db,
        &[running],
        &[Running {
            worktree: "/d/bae-worktrees/side-1".into(),
            branch: "b".into(),
            brief: "p".into(),
            since: stamp(NOON),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
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
    close(&db, q, How::Done("shown".into()), &stamp(NOON)).unwrap();
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

#[test]
fn a_running_last_item_has_no_priority_wait() {
    let db = db();
    let last = item(&db, vec![]);
    priority(&db, last, Level::Last);
    crate::lists::running::start(
        &db,
        &[last],
        &[Running {
            worktree: "/d/bae-worktrees/side-1".into(),
            branch: "work".into(),
            brief: "brief".into(),
            since: stamp(NOON),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
    item(&db, vec![]);
    let lists = read(&db);
    assert!(!waits_for_others(&lists, lists.item(last).unwrap()));
}

fn order(db: &Connection) -> Vec<(u32, usize)> {
    stack_order(&read(db))
        .iter()
        .map(|e| (e.question.id.0, e.blocked_count))
        .collect()
}

fn blocked_question(db: &Connection, blocks: usize) -> Id {
    let q = question(db);
    for _ in 0..blocks {
        item(db, vec![Wait::Question(q)]);
    }
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
    assert_eq!(
        entry
            .blocks
            .iter()
            .map(|b| (b.item.id, b.behind))
            .collect::<Vec<_>>(),
        vec![(direct, 3)]
    );
    // A closed item in the chain no longer counts.
    close(
        &db,
        Id(3),
        How::Landed(vec![crate::lists::fixtures::sha('a')]),
        &stamp(NOON),
    )
    .unwrap();
    assert_eq!(order(&db)[0], (q.0, 1 + 2));
}

#[test]
fn the_stack_orders_ties_in_what_they_unblock_by_effect_then_oldest_first() {
    let db = db();
    let cleanup = question_effect(&db, Effect::Cleanup);
    let design = question_effect(&db, Effect::ProductDesign);
    let loss = question_effect(&db, Effect::DataLoss);
    let design_again = question_effect(&db, Effect::ProductDesign);
    let blocking = blocked_question(&db, 1);
    let ids: Vec<Id> = stack_order(&read(&db))
        .iter()
        .map(|e| e.question.id)
        .collect();
    assert_eq!(ids, vec![blocking, loss, design, design_again, cleanup]);
}

#[test]
fn placements_sit_outside_the_count() {
    let db = db();
    let big = blocked_question(&db, 3);
    let small = blocked_question(&db, 1);
    let first_old = question(&db);
    let first_new = question(&db);
    let last = blocked_question(&db, 5);
    let loss = question_effect(&db, Effect::DataLoss);
    place(&db, first_new, Spot::First);
    place(&db, first_old, Spot::First);
    place(&db, last, Spot::Last);
    place(&db, loss, Spot::Last);
    let ids: Vec<Id> = stack_order(&read(&db))
        .iter()
        .map(|e| e.question.id)
        .collect();
    // The stack puts the questions placed first on top, newest placement
    // first, whatever they block and whenever someone asked them. Then it
    // orders the rest by count and puts the questions placed last at the end,
    // whatever their effect.
    assert_eq!(ids, vec![first_old, first_new, big, small, last, loss]);
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
    let ids: Vec<Id> = dispatch_order(&read(&db), Audience::Coordinator)
        .iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        ids,
        vec![first_early, first_late, alpha_old, alpha_new, beta_old]
    );
    // A last item is ready only once the others are gone, and then comes out.
    let last = item(&db, vec![]);
    priority(&db, last, Level::Last);
    assert!(
        !dispatch_order(&read(&db), Audience::Coordinator)
            .iter()
            .any(|i| i.id == last)
    );
    for id in [
        first_early,
        first_late,
        alpha_old,
        alpha_new,
        beta_old,
        waiting,
    ] {
        close(&db, id, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    }
    assert_eq!(
        dispatch_order(&read(&db), Audience::Coordinator)
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
        vec![last]
    );
}

#[test]
fn dispatch_order_puts_what_other_items_wait_on_ahead_of_area_and_age() {
    let db = db();
    let alpha_old = item_opened(&db, vec![], stamp(NOON - 100));
    let beta_blocker = item(&db, vec![]);
    set_area(&db, beta_blocker, "Beta");
    let beta_one = item(&db, vec![Wait::Item(beta_blocker)]);
    let _beta_two = item(&db, vec![Wait::Item(beta_one)]);
    let beta_small = item(&db, vec![]);
    set_area(&db, beta_small, "Beta");
    let _waits_on_small = item(&db, vec![Wait::Item(beta_small)]);
    let first = item_opened(&db, vec![], stamp(NOON + 900));
    priority(&db, first, Level::First);
    let ids: Vec<Id> = dispatch_order(&read(&db), Audience::Coordinator)
        .iter()
        .map(|i| i.id)
        .collect();
    // First comes first; then 2 items wait behind the blocker (through a
    // chain), 1 behind the small one, and none behind the old Alpha item.
    assert_eq!(ids, vec![first, beta_blocker, beta_small, alpha_old]);
}

#[test]
fn the_design_session_gets_its_ready_items_in_the_same_order() {
    let db = db();
    let q = question(&db);
    let plain = item_opened(&db, vec![], stamp(NOON - 100));
    set_audience(&db, plain, Audience::DesignSession);
    let blocker = item(&db, vec![]);
    set_audience(&db, blocker, Audience::DesignSession);
    let _app_work = item(&db, vec![Wait::Item(blocker)]);
    let waiting = item(&db, vec![Wait::Question(q)]);
    set_audience(&db, waiting, Audience::DesignSession);
    let coordinator = item(&db, vec![]);
    let ids: Vec<Id> = dispatch_order(&read(&db), Audience::DesignSession)
        .iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(ids, vec![blocker, plain]);
    assert!(!ids.contains(&coordinator) && !ids.contains(&waiting));
    // Placed last, a design item waits for the design session's other open
    // items, not the coordinator's.
    let last = item(&db, vec![]);
    set_audience(&db, last, Audience::DesignSession);
    priority(&db, last, Level::Last);
    let ids: Vec<Id> = dispatch_order(&read(&db), Audience::DesignSession)
        .iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(ids, vec![blocker, plain]);
}

#[test]
fn a_total_counts_a_moment_two_spans_cover_once_and_the_open_span_from_its_start() {
    let total = Total::of([
        (10, Some(40)),
        (30, Some(50)),
        (100, Some(130)),
        (120, None),
    ]);
    assert_eq!(
        total,
        Total {
            seconds: 60,
            since: Some(120)
        }
    );
    assert_eq!(total.at(200), 140);
    assert_eq!(Total::of([]).at(200), 0);
}

fn run_in(db: &Connection, id: Id, worktree: &str, branch: &str, at: i64) {
    crate::lists::running::start(
        db,
        &[id],
        &[Running {
            worktree: worktree.into(),
            branch: branch.into(),
            brief: "b.md".into(),
            since: stamp(at),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
}

#[test]
fn times_add_up_an_items_runs_and_spans_and_a_branch_counts_its_items_runs_once() {
    use crate::lists::running::{end, open_span};
    let db = db();
    let first = item(&db, vec![]);
    let second = item(&db, vec![]);
    let side = "/d/bae-worktrees/side-1";
    run_in(&db, first, side, "work", NOON);
    open_span(&db, first, SpanKind::Held, NOON + 100).unwrap();
    end(&db, first, Ended::Stopped, &stamp(NOON + 300)).unwrap();
    run_in(&db, first, side, "work", NOON + 1000);
    run_in(&db, second, side, "work", NOON + 1000);
    open_span(&db, first, SpanKind::Waiting, NOON + 1100).unwrap();
    end(&db, second, Ended::Closed, &stamp(NOON + 1500)).unwrap();
    close(
        &db,
        second,
        How::Landed(vec!["a".repeat(40)]),
        &stamp(NOON + 1500),
    )
    .unwrap();
    let lists = read(&db);
    let times = item_times(&lists, &lists.items[0], None).unwrap().unwrap();
    assert_eq!(
        times,
        Times {
            first: NOON,
            runs: 2,
            ran: Total {
                seconds: 300,
                since: Some(NOON + 1000)
            },
            held: Total {
                seconds: 200,
                since: None
            },
            waiting: Total {
                seconds: 0,
                since: Some(NOON + 1100)
            },
            landed: None,
            spans_from: None,
        }
    );
    let branch = &branch_times(&lists).unwrap()[&("bae".to_string(), "work".to_string())];
    assert_eq!(
        (branch.runs, branch.ran, branch.landed),
        (3, times.ran, None)
    );
    end(&db, first, Ended::Closed, &stamp(NOON + 2000)).unwrap();
    close(
        &db,
        first,
        How::Landed(vec!["b".repeat(40)]),
        &stamp(NOON + 2000),
    )
    .unwrap();
    let lists = read(&db);
    let branch = &branch_times(&lists).unwrap()[&("bae".to_string(), "work".to_string())];
    assert_eq!(
        (branch.ran.at(NOON + 9999), branch.landed),
        (1300, Some(NOON + 2000))
    );
}

#[test]
fn a_span_that_lies_in_none_of_its_items_runs_is_an_error() {
    use crate::lists::fixtures::v;
    let db = db();
    let id = item(&db, vec![]);
    run_in(&db, id, "/d/bae-worktrees/side-1", "work", NOON);
    crate::lists::running::end(&db, id, Ended::Stopped, &stamp(NOON + 100)).unwrap();
    crate::lists::running::plant(
        &db,
        "item_span",
        &[
            ("item_id", v(id.0)),
            ("kind", v("held")),
            ("since_at", v(NOON + 200)),
            ("ended_at", v(NOON + 300)),
        ],
    )
    .unwrap();
    let lists = read(&db);
    let fault = format!(
        "item {id} has a held span from {} that lies in none of its runs",
        NOON + 200
    );
    let message = |result: Result<_>| match result {
        Err(crate::fail::Fail::Message(message)) => message,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        message(item_times(&lists, &lists.items[0], None).map(|_| ())),
        fault
    );
    assert_eq!(message(branch_times(&lists).map(|_| ())), fault);
}
