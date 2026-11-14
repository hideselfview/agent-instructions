use super::*;
use crate::lists::Running;
use crate::lists::Wait;
use crate::lists::fixtures::{NOON, blocks_something, db, design, item, question, read, stamp};
use crate::lists::view::ClaimFacts;

fn start(db: &rusqlite::Connection, item: Id, checkout: &str) {
    crate::lists::running::start(
        db,
        &[item],
        &[Running {
            worktree: format!("/d/bae-worktrees/{checkout}"),
            branch: format!("{checkout}-branch"),
            brief: "plans/briefs/x.md".into(),
            since: stamp(NOON),
        }],
    )
    .unwrap();
}

fn facts(note_state: Option<&str>) -> ClaimFacts {
    ClaimFacts {
        agent: Some("agent".into()),
        note: None,
        note_state: note_state.map(str::to_string),
        waiting_to_land: 0,
    }
}

fn board_of(db: &rusqlite::Connection, claims: &[(&str, Option<&str>)]) -> Board {
    let mut view = View::build(&read(db), NOON);
    for item in &mut view.items {
        for running in &mut item.running {
            let state = claims
                .iter()
                .find(|(checkout, _)| *checkout == running.checkout)
                .and_then(|(_, state)| *state);
            running.claim = Some(facts(state));
        }
    }
    build(&view)
}

#[test]
fn each_tab_counts_what_it_holds() {
    let db = db();
    // Waiting on you: a question, a design change to review, a worktree whose
    // note says so, and one whose item waits on a question.
    let asked = question(&db);
    blocks_something(&db, asked);
    design(&db);
    let noted = item(&db, vec![]);
    start(&db, noted, "side-1");
    let held = item(&db, vec![Wait::Question(asked)]);
    start(&db, held, "side-2");
    // Running: one worktree with two items counts once.
    let first = item(&db, vec![]);
    let second = item(&db, vec![]);
    start(&db, first, "side-3");
    start(&db, second, "side-3");
    // Queue: items no agent has started.
    item(&db, vec![]);
    item(&db, vec![Wait::Condition("a quiet machine".into())]);

    let board = board_of(&db, &[("side-1", Some("waiting-on-you"))]);
    assert_eq!(
        (board.waiting, board.running, board.queue),
        (1 + 1 + 2, 1, 2)
    );
    let tab_of = |checkout: &str| {
        board
            .works
            .iter()
            .find(|work| work.checkout == checkout)
            .unwrap()
            .tab
    };
    assert_eq!(tab_of("side-1"), Tab::Waiting);
    assert_eq!(tab_of("side-2"), Tab::Waiting);
    assert_eq!(tab_of("side-3"), Tab::Running);
    let shared = board.works.iter().find(|w| w.checkout == "side-3").unwrap();
    assert_eq!(shared.items, vec![first, second]);
}

#[test]
fn a_design_change_waits_on_review_until_approved_and_then_runs() {
    let db = db();
    let id = design(&db);
    let before = board_of(&db, &[]);
    assert_eq!((before.waiting, before.running), (1, 0));
    crate::lists::design::approve(&db, id, &stamp(NOON)).unwrap();
    let after = board_of(&db, &[]);
    assert_eq!((after.waiting, after.running), (0, 1));
}

#[test]
fn a_wait_on_another_item_is_not_the_persons() {
    let db = db();
    let other = item(&db, vec![]);
    let waits = item(&db, vec![Wait::Item(other)]);
    start(&db, waits, "side-1");
    let board = board_of(&db, &[]);
    assert_eq!((board.waiting, board.running), (0, 1));
}
