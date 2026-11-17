use super::*;
use crate::lists::fixtures::{NOON, db, item, read, stamp};
use crate::lists::view::ClaimFacts;
use crate::lists::{Id, Running};

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

#[test]
fn a_worktree_with_two_items_is_one_work() {
    let db = db();
    let first = item(&db, vec![]);
    let second = item(&db, vec![]);
    start(&db, first, "side-3");
    start(&db, second, "side-3");
    let works = works(&View::build(&read(&db), NOON).unwrap());
    assert_eq!(works.len(), 1);
    assert_eq!(
        works[0].items.iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![first, second]
    );
}

#[test]
fn a_work_says_what_it_is() {
    let db = db();
    let ready = item(&db, vec![]);
    start(&db, ready, "side-1");
    crate::lists::landing::add_part(
        &db,
        ready,
        crate::lists::LandedPart {
            repo: "bae".into(),
            commits: vec![
                "c562692e826cd7a1b2c3d4e5f60718293a4b5c6d".into(),
                "6848a15b2c3d4e5f60718293a4b5c6d7e8f90a1b".into(),
            ],
            at: NOON,
        },
    )
    .unwrap();
    let working = item(&db, vec![]);
    start(&db, working, "side-2");
    let held = item(&db, vec![]);
    start(&db, held, "side-3");
    let mut view = View::build(&read(&db), NOON).unwrap();
    for item in &mut view.items {
        for running in &mut item.running {
            running.claim = Some(ClaimFacts {
                agent: Some("circle-raise".into()),
                note: Some("fixing the tests".into()),
                note_state: None,
                waiting_to_land: 2,
            });
        }
    }
    let works = works(&view);
    let first = works.iter().find(|w| w.checkout == "side-1").unwrap();
    assert_eq!(first.title, read(&db).item(ready).unwrap().title);
    assert_eq!(first.now.as_deref(), Some("fixing the tests"));
    assert_eq!(first.agent.as_deref(), Some("circle-raise"));
    assert_eq!(first.branch, "side-1-branch");
    assert_eq!(first.plan, "plans/briefs/x.md");
    assert_eq!(
        first.landed,
        vec!["c562692".to_string(), "6848a15".to_string()]
    );
}

/// The works of a worktree running two items, claimed under <agent>, and the
/// first item's title.
fn two_items_claimed_by(agent: Option<&str>) -> (Vec<Work>, String) {
    let db = db();
    let first = item(&db, vec![]);
    let second = item(&db, vec![]);
    start(&db, first, "side-3");
    start(&db, second, "side-3");
    let mut view = View::build(&read(&db), NOON).unwrap();
    for item in &mut view.items {
        for running in &mut item.running {
            running.claim = Some(ClaimFacts {
                agent: agent.map(str::to_string),
                note: None,
                note_state: None,
                waiting_to_land: 0,
            });
        }
    }
    (works(&view), read(&db).item(first).unwrap().title.clone())
}

#[test]
fn a_work_of_several_items_is_titled_by_its_agents_job() {
    let (works, _) = two_items_claimed_by(Some("screen reads library agent"));
    assert_eq!(works[0].title, "screen reads library");
    assert_eq!(
        works[0].agent.as_deref(),
        Some("screen reads library agent")
    );
}

#[test]
fn a_work_of_several_items_without_an_agent_name_takes_its_first_items_title() {
    let (works, first_title) = two_items_claimed_by(None);
    assert_eq!(works[0].title, first_title);
}

fn facts_of(note_state: Option<&str>) -> ClaimFacts {
    ClaimFacts {
        agent: Some("agent".into()),
        note: None,
        note_state: note_state.map(str::to_string),
        waiting_to_land: 0,
    }
}

fn board_of(db: &rusqlite::Connection, claims: &[(&str, Option<&str>)]) -> Board {
    let mut view = View::build(&read(db), NOON).unwrap();
    for item in &mut view.items {
        for running in &mut item.running {
            let state = claims
                .iter()
                .find(|(checkout, _)| *checkout == running.checkout)
                .and_then(|(_, state)| *state);
            running.claim = Some(facts_of(state));
        }
    }
    board(&view)
}

#[test]
fn each_tab_counts_what_it_holds() {
    use crate::lists::Wait;
    use crate::lists::fixtures::{design, question};
    let db = db();
    // Waiting on you: a question, a design change to review, a worktree whose
    // note says so, and one whose item waits on a question.
    let asked = question(&db);
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
    // Queue: items nobody has started.
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
}

#[test]
fn a_design_change_waits_on_review_until_approved_and_then_runs() {
    use crate::lists::fixtures::design;
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
    use crate::lists::Wait;
    let db = db();
    let other = item(&db, vec![]);
    let waits = item(&db, vec![Wait::Item(other)]);
    start(&db, waits, "side-1");
    let board = board_of(&db, &[]);
    assert_eq!((board.waiting, board.running, board.queue), (0, 1, 1));
}

#[test]
fn work_whose_item_waits_on_a_design_change_to_review_is_held_on_the_person() {
    use crate::lists::Wait;
    use crate::lists::fixtures::design;
    let db = db();
    let change = design(&db);
    let waits = item(&db, vec![Wait::Design(change)]);
    start(&db, waits, "side-1");
    let board = board_of(&db, &[]);
    assert_eq!(board.works[0].tab, Tab::Waiting);
    // The change to review and the work it holds.
    assert_eq!((board.waiting, board.running), (2, 0));
}

#[test]
fn work_whose_item_waits_on_an_approved_design_change_is_not_held_on_the_person() {
    use crate::lists::Wait;
    use crate::lists::fixtures::design;
    let db = db();
    let change = design(&db);
    let waits = item(&db, vec![Wait::Design(change)]);
    start(&db, waits, "side-1");
    crate::lists::design::approve(&db, change, &stamp(NOON)).unwrap();
    let board = board_of(&db, &[]);
    assert_eq!(board.works[0].tab, Tab::Running);
    // The work, and the approved change that waits to land.
    assert_eq!((board.waiting, board.running), (0, 2));
}
