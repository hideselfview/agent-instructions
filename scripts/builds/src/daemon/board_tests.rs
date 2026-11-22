use super::*;
use crate::lists::fixtures::{NOON, db, item, read, stamp};
use crate::lists::view::{ClaimFacts, RulesReview};
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
    board_reviewed(db, claims, RulesReview::Read)
}

/// The board of <db>'s lists, with each running worktree's claim note state
/// in <claims> and the rules review of every design change that waits on
/// the person's approval standing at <review>, as the daemon joins them.
fn board_reviewed(
    db: &rusqlite::Connection,
    claims: &[(&str, Option<&str>)],
    review: RulesReview,
) -> Board {
    let mut view = View::build(&read(db), NOON).unwrap();
    for design in &mut view.designs {
        if design.closed.is_none() && design.approved.is_none() {
            design.rules_review = Some(review.clone());
        }
    }
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
    item(
        &db,
        vec![Wait::condition(
            "a quiet machine",
            crate::lists::Ender::Coordinator,
        )],
    );

    let board = board_of(&db, &[("side-1", Some("waiting-on-you"))]);
    assert_eq!(board.waiting, 1 + 1 + 2);
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
fn a_review_waiting_for_permission_stays_on_the_board_until_its_retry() {
    use crate::lists::fixtures::make_store;
    use crate::lists::item_fixtures::words;
    use crate::lists::question_command::run;
    use crate::testing::Scratch;

    let scratch = Scratch::new("board-review-permission");
    let tool = scratch.tool(0);
    let store = make_store(&tool.places, &["bae"]);
    let item = store
        .write(&stamp(NOON), "start the review item", |db| {
            let item = item(db, vec![]);
            start(db, item, "side-1");
            Ok(item)
        })
        .unwrap();
    let command = |args: &[&str]| {
        run(&tool, &words(args), &scratch.dir, &|_| {
            panic!("a local review permission wait must not call GitHub")
        })
        .unwrap()
    };
    let details = scratch.dir.join("review-permission.md");
    std::fs::write(
        &details,
        "The reviewer needs permission to read the branch diff.",
    )
    .unwrap();
    command(&[
        "ask",
        "--kind",
        "your-step",
        "--effect",
        "slows-agents",
        "--title",
        "Approve the branch review",
        "--details-file",
        details.to_str().unwrap(),
        "--blocks",
        &item.0.to_string(),
        "--first",
        "The review blocks dispatch.",
    ]);
    let blocked = store.load().unwrap();
    let question = blocked.questions[0].id;
    let waiting = board(&View::build(&blocked, NOON).unwrap());
    assert_eq!(waiting.waiting, 2);
    assert_eq!(waiting.works[0].tab, Tab::Waiting);

    // A second refusal uses the question already linked from the work item.
    assert_eq!(
        blocked.item(item).unwrap().waits,
        [crate::lists::Wait::Question(question)]
    );
    command(&["show", &question.0.to_string()]);
    let still_waiting = store.load().unwrap();
    assert_eq!(still_waiting.questions.len(), 1);
    assert_eq!(
        board(&View::build(&still_waiting, NOON).unwrap()).waiting,
        2
    );

    let done = scratch.dir.join("review-started.json");
    std::fs::write(
        &done,
        serde_json::json!({
            "shown_by": "The retried review reached the reviewer.",
            "blocked": {item.0.to_string(): "proceeds"},
            "no_work": "The existing review continues."
        })
        .to_string(),
    )
    .unwrap();
    command(&[
        "done",
        &question.0.to_string(),
        "--request",
        done.to_str().unwrap(),
    ]);
    let resumed = store.load().unwrap();
    let running = board(&View::build(&resumed, NOON).unwrap());
    assert_eq!(running.waiting, 0);
    assert_eq!(running.works[0].tab, Tab::Running);
}

#[test]
fn a_design_change_waits_on_review_until_approved_and_then_runs() {
    use crate::lists::fixtures::design;
    let db = db();
    let id = design(&db);
    let before = board_of(&db, &[]);
    assert_eq!(before.waiting, 1);
    crate::lists::design::approve(&db, id, &stamp(NOON)).unwrap();
    let after = board_of(&db, &[]);
    assert_eq!(after.waiting, 0);
}

/// A design change waits on the person's review only once its rules review
/// takes its branch (decision #362).
#[test]
fn a_design_change_in_its_rules_review_is_not_under_waiting_on_you() {
    use crate::lists::fixtures::design;
    let db = db();
    design(&db);
    assert_eq!(board_reviewed(&db, &[], RulesReview::InReview).waiting, 0);
    assert_eq!(board_reviewed(&db, &[], RulesReview::Read).waiting, 1);
}

#[test]
fn a_wait_on_another_item_is_not_the_persons() {
    use crate::lists::Wait;
    let db = db();
    let other = item(&db, vec![]);
    let waits = item(&db, vec![Wait::Item(other)]);
    start(&db, waits, "side-1");
    let board = board_of(&db, &[]);
    assert_eq!(board.waiting, 0);
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
    assert_eq!(board.waiting, 2);
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
    assert_eq!(board.waiting, 0);
}

#[test]
fn a_work_carries_the_times_of_its_branch() {
    let db = db();
    let earlier = item(&db, vec![]);
    let now = item(&db, vec![]);
    start(&db, earlier, "side-4");
    crate::lists::running::end(
        &db,
        earlier,
        crate::lists::Ended::Stopped,
        &stamp(NOON + 600),
    )
    .unwrap();
    crate::lists::running::start(
        &db,
        &[now],
        &[Running {
            worktree: "/d/bae-worktrees/side-4".into(),
            branch: "side-4-branch".into(),
            brief: "plans/briefs/x.md".into(),
            since: stamp(NOON + 900),
        }],
    )
    .unwrap();
    let works = works(&View::build(&read(&db), NOON + 1000).unwrap());
    assert_eq!(
        (
            works[0].times.first,
            works[0].times.runs,
            works[0].times.ran
        ),
        (
            NOON,
            2,
            crate::lists::derive::Total {
                seconds: 600,
                since: Some(NOON + 900)
            }
        )
    );
}
