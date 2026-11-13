use super::*;
use crate::lists::fixtures::{NOON, blocks_something, db, item, question, sha, stamp};
use crate::lists::item::{add_wait, remove_wait};
use crate::lists::landing::add_part;
use crate::lists::record::close;
use crate::lists::running::{Running, start};
use crate::lists::{How, LandedPart, Wait};

fn faults_of(db: &Connection) -> Vec<String> {
    faults(db, "demo").unwrap()
}

fn running(worktree: &str, branch: &str) -> Running {
    Running { worktree: worktree.into(), branch: branch.into(), brief: "plans/b.md".into(), since: stamp(NOON) }
}

#[test]
fn a_store_that_holds_everything_it_must_has_no_fault() {
    let db = db();
    let q = question(&db);
    let a = item(&db, vec![Wait::Question(q)]);
    let b = item(&db, vec![Wait::Item(a)]);
    blocks_something(&db, q);
    start(&db, &[b], &[running("/d/bae-worktrees/side-1", "work")]).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
}

#[test]
fn a_project_without_repos_is_a_fault() {
    let db = crate::db::memory();
    crate::lists::project::init(
        &db,
        &crate::lists::project::NewProject {
            name: "demo".into(),
            repos: vec![],
            places: Default::default(),
            sources: Default::default(),
            specs: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(faults_of(&db), ["the project has no repos"]);
}

#[test]
fn a_question_that_blocks_nothing_and_says_nothing_of_why_is_a_fault_until_an_open_item_waits_on_it() {
    let db = db();
    let q = question(&db);
    blocks_something(&db, q);
    assert_eq!(faults_of(&db).len(), 1);
    assert!(faults_of(&db)[0].contains(&format!("question {q} blocks no open item")), "{:?}", faults_of(&db));
    let waiting = item(&db, vec![Wait::Question(q)]);
    assert_eq!(faults_of(&db), Vec::<String>::new());
    // A closed item does not count as blocked.
    close(&db, waiting, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    assert_eq!(faults_of(&db).len(), 1);
}

#[test]
fn an_open_items_wait_on_a_record_closed_so_it_never_meets_it_is_a_fault() {
    let db = db();
    let dropped_q = question(&db);
    let answered_q = question(&db);
    let dropped_item = item(&db, vec![]);
    let landed_item = item(&db, vec![]);
    let waiter = item(&db, vec![Wait::Question(dropped_q), Wait::Question(answered_q), Wait::Item(dropped_item), Wait::Item(landed_item)]);
    close(&db, dropped_q, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    close(&db, answered_q, How::Done, &stamp(NOON)).unwrap();
    close(&db, dropped_item, How::Deferred("https://x/1".into()), &stamp(NOON)).unwrap();
    close(&db, landed_item, How::Landed(vec![sha('a')]), &stamp(NOON)).unwrap();
    let found = faults_of(&db);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains(&format!("item {waiter} waits on {dropped_q}")) && found[0].contains("closed as dropped"), "{found:?}");
    assert!(found[1].contains(&format!("waits on {dropped_item}")) && found[1].contains("closed as deferred"), "{found:?}");
    // The same waits of a closed item are history, not a fault.
    close(&db, waiter, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
}

#[test]
fn open_items_that_wait_on_each_other_in_a_circle_are_a_fault_and_a_closed_one_breaks_it() {
    let db = db();
    let a = item(&db, vec![]);
    let b = item(&db, vec![Wait::Item(a)]);
    let c = item(&db, vec![Wait::Item(b)]);
    assert_eq!(faults_of(&db), Vec::<String>::new());
    add_wait(&db, a, Wait::Item(c)).unwrap();
    let found = faults_of(&db);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("items wait on each other in a circle: ") && found[0].contains("->"), "{found:?}");
    for id in [a, b, c] {
        assert!(found[0].contains(&id.to_string()), "{found:?}");
    }
    remove_wait(&db, a, &Wait::Item(c)).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
    // A shorter circle is found too.
    add_wait(&db, a, Wait::Item(b)).unwrap();
    assert!(faults_of(&db)[0].contains(&format!("{a} -> {b} -> {a}")), "{:?}", faults_of(&db));
}

#[test]
fn a_closed_item_that_still_runs_somewhere_is_a_fault() {
    let db = db();
    let id = item(&db, vec![]);
    start(&db, &[id], &[running("/d/bae-worktrees/side-1", "work")]).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
    close(&db, id, How::Landed(vec![sha('a')]), &stamp(NOON)).unwrap();
    let found = faults_of(&db);
    assert_eq!(found, [format!("item {id} is closed and still runs somewhere")]);
}

#[test]
fn a_worktree_that_holds_two_branches_of_work_is_a_fault_and_one_branch_for_two_items_is_not() {
    let db = db();
    let a = item(&db, vec![]);
    let b = item(&db, vec![]);
    let c = item(&db, vec![]);
    start(&db, &[a], &[running("/d/bae-worktrees/side-1", "work")]).unwrap();
    start(&db, &[b], &[running("/d/bae-worktrees/side-1", "work")]).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
    start(&db, &[c], &[running("/d/bae-worktrees/side-1", "other")]).unwrap();
    let found = faults_of(&db);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("/d/bae-worktrees/side-1 holds two branches of work") && found[0].contains("work") && found[0].contains("other"), "{found:?}");
}

#[test]
fn a_landed_closing_or_a_landed_part_with_no_commit_is_a_fault() {
    let db = db();
    let a = item(&db, vec![]);
    let b = item(&db, vec![]);
    add_part(&db, a, LandedPart { repo: "bae".into(), commits: vec![], at: 1 }).unwrap();
    close(&db, b, How::Landed(vec![]), &stamp(NOON)).unwrap();
    let found = faults_of(&db);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().any(|f| f == &format!("closed item {b} landed with no commits")), "{found:?}");
    assert!(found.iter().any(|f| f == &format!("item {a} has a landed part of repo bae with no commits")), "{found:?}");
}

#[test]
fn a_project_row_that_names_another_folders_project_is_a_fault() {
    let db = db();
    assert_eq!(faults(&db, "demo").unwrap(), Vec::<String>::new());
    assert_eq!(faults(&db, "other").unwrap(), ["the project row names demo, and the folder is other's"]);
}

#[test]
fn opening_a_store_with_a_planted_fault_names_it_in_every_command_and_repairs_nothing() {
    use crate::testing::Scratch;
    let scratch = Scratch::new("check-open");
    let store = crate::lists::fixtures::make_store(&scratch.places(), &["bae", "coven"]);
    {
        let db = crate::db::open(&store.database(), crate::db::Make::Never).unwrap();
        let a = item(&db, vec![]);
        let b = item(&db, vec![Wait::Item(a)]);
        add_wait(&db, a, Wait::Item(b)).unwrap();
        let q = question(&db);
        blocks_something(&db, q);
    }
    let before = crate::lists::fixtures::state_of(&store);
    let loaded = store.load().unwrap_err().text();
    assert!(loaded.contains("fail their check") && loaded.contains("in a circle") && loaded.contains("blocks no open item"), "{loaded}");
    let written = store.write(&stamp(NOON), "w", |_| Ok(())).unwrap_err().text();
    assert!(written.contains("in a circle") && written.contains("blocks no open item"), "{written}");
    assert_eq!(crate::lists::fixtures::state_of(&store), before, "nothing repaired it");
}

#[test]
fn a_write_that_breaks_a_rule_is_refused_whole_and_naming_the_fault() {
    use crate::testing::Scratch;
    let scratch = Scratch::new("check-write");
    let store = crate::lists::fixtures::make_store(&scratch.places(), &["bae", "coven"]);
    let before = crate::lists::fixtures::state_of(&store);
    let refused = store
        .write(&stamp(NOON), "w", |db| {
            let id = item(db, vec![]);
            start(db, &[id], &[running("/d/bae-worktrees/side-1", "work")])?;
            close(db, id, How::Landed(vec![sha('a')]), &stamp(NOON))
        })
        .unwrap_err()
        .text();
    assert!(refused.contains("is closed and still runs somewhere"), "{refused}");
    assert_eq!(crate::lists::fixtures::state_of(&store), before);
}
