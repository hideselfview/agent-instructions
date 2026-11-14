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
    let works = works(&View::build(&read(&db), NOON));
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
    let mut view = View::build(&read(&db), NOON);
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
