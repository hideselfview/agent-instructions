use super::*;
use crate::lists::fixtures::{NOON, blocks_something, db, item, question, sha, stamp};
use crate::lists::item::{add_wait, remove_wait};
use crate::lists::landing::add_part;
use crate::lists::record::close;
use crate::lists::running::{Ended, Running, start};
use crate::lists::{How, LandedPart, Wait};

fn faults_of(db: &Connection) -> Vec<String> {
    faults(db, "demo").unwrap()
}

fn running(worktree: &str, branch: &str) -> Running {
    Running {
        worktree: worktree.into(),
        branch: branch.into(),
        brief: "plans/b.md".into(),
        since: stamp(NOON),
    }
}

#[test]
fn a_store_that_holds_everything_it_must_has_no_fault() {
    let db = db();
    let q = question(&db);
    let a = item(&db, vec![Wait::Question(q)]);
    let b = item(&db, vec![Wait::Item(a)]);
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
            canvas: false,
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
fn a_question_that_blocks_nothing_and_says_nothing_of_why_is_a_fault_until_an_open_item_waits_on_it()
 {
    let db = db();
    let q = question(&db);
    blocks_something(&db, q);
    assert_eq!(faults_of(&db).len(), 1);
    assert!(
        faults_of(&db)[0].contains(&format!("question {q} blocks no open item")),
        "{:?}",
        faults_of(&db)
    );
    let waiting = item(&db, vec![Wait::Question(q)]);
    assert_eq!(faults_of(&db), Vec::<String>::new());
    // A closed item does not count as blocked.
    close(&db, waiting, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    assert_eq!(faults_of(&db).len(), 1);
}

#[test]
fn a_question_that_says_why_it_blocks_nothing_while_an_open_item_waits_on_it_is_a_fault() {
    let db = db();
    let q = question(&db);
    let gone = item(&db, vec![Wait::Question(q)]);
    close(&db, gone, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    crate::lists::fixtures::blocks_nothing_because(&db, q, "asked anyway");
    // A closed item's wait does not count.
    assert_eq!(faults_of(&db), Vec::<String>::new());
    let waiting = item(&db, vec![]);
    add_wait(&db, waiting, Wait::Question(q)).unwrap();
    crate::lists::fixtures::blocks_nothing_because(&db, q, "asked anyway");
    assert_eq!(
        faults_of(&db),
        [format!(
            "question {q} gives a reason it blocks nothing (asked anyway) while open item {waiting} waits on it: \
             a question gives that reason only while no open item waits on it, and \
             `builds item wait ITEM --off question:{} --blocks-nothing WHY` gives the reason in the write that removes the last open item's wait on it",
            q.0
        )]
    );
}

#[test]
fn an_open_items_wait_on_a_record_closed_so_it_never_meets_it_is_a_fault() {
    let db = db();
    let dropped_q = question(&db);
    let answered_q = question(&db);
    let dropped_item = item(&db, vec![]);
    let landed_item = item(&db, vec![]);
    let waiter = item(
        &db,
        vec![
            Wait::Question(dropped_q),
            Wait::Question(answered_q),
            Wait::Item(dropped_item),
            Wait::Item(landed_item),
        ],
    );
    close(&db, dropped_q, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    close(&db, answered_q, How::Done, &stamp(NOON)).unwrap();
    close(
        &db,
        dropped_item,
        How::Deferred("https://x/1".into()),
        &stamp(NOON),
    )
    .unwrap();
    close(&db, landed_item, How::Landed(vec![sha('a')]), &stamp(NOON)).unwrap();
    let found = faults_of(&db);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found[0].contains(&format!("item {waiter} waits on {dropped_q}"))
            && found[0].contains("closed as dropped"),
        "{found:?}"
    );
    assert!(
        found[1].contains(&format!("waits on {dropped_item}"))
            && found[1].contains("closed as deferred"),
        "{found:?}"
    );
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
    assert!(
        found[0].starts_with("items wait on each other in a circle: ") && found[0].contains("->"),
        "{found:?}"
    );
    for id in [a, b, c] {
        assert!(found[0].contains(&id.to_string()), "{found:?}");
    }
    remove_wait(&db, a, &Wait::Item(c)).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
    // A shorter circle is found too.
    add_wait(&db, a, Wait::Item(b)).unwrap();
    assert!(
        faults_of(&db)[0].contains(&format!("{a} -> {b} -> {a}")),
        "{:?}",
        faults_of(&db)
    );
}

#[test]
fn a_closed_item_that_still_runs_somewhere_is_a_fault() {
    let db = db();
    let id = item(&db, vec![]);
    start(&db, &[id], &[running("/d/bae-worktrees/side-1", "work")]).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
    close(&db, id, How::Landed(vec![sha('a')]), &stamp(NOON)).unwrap();
    let found = faults_of(&db);
    assert_eq!(
        found,
        [format!("item {id} is closed and still runs somewhere")]
    );
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
    assert!(
        found[0].contains("/d/bae-worktrees/side-1 holds two branches of work")
            && found[0].contains("work")
            && found[0].contains("other"),
        "{found:?}"
    );
}

fn kept(worktree: &str, branch: &str) -> crate::lists::kept_work::KeptWork {
    crate::lists::kept_work::KeptWork {
        worktree: worktree.into(),
        branch: branch.into(),
        since: stamp(NOON),
    }
}

/// An item that keeps a worktree holds its worktree alone and is never running or closed.
#[test]
fn an_item_that_keeps_a_worktree_that_shares_its_worktree_runs_or_is_closed_is_a_fault() {
    use crate::lists::kept_work::keep;
    let db = db();
    let a = item(&db, vec![]);
    let b = item(&db, vec![]);
    keep(&db, a, &[kept("/d/bae-worktrees/side-1", "work")]).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
    start(&db, &[b], &[running("/d/bae-worktrees/side-1", "work")]).unwrap();
    assert_eq!(
        faults_of(&db),
        [format!(
            "worktree /d/bae-worktrees/side-1 holds the kept work of item {a} and item {b}"
        )]
    );
    crate::lists::running::end(&db, b, Ended::Stopped, &stamp(NOON)).unwrap();
    start(&db, &[a], &[running("/d/bae-worktrees/side-1", "work")]).unwrap();
    assert_eq!(
        faults_of(&db),
        [format!("item {a} both runs and keeps a worktree")]
    );
    crate::lists::running::end(&db, a, Ended::Closed, &stamp(NOON)).unwrap();
    close(&db, a, How::Landed(vec![sha('a')]), &stamp(NOON)).unwrap();
    assert_eq!(
        faults_of(&db),
        [format!("item {a} is closed and still keeps a worktree")]
    );
}

/// A kept branch counts as the worktree's one branch of work.
#[test]
fn a_kept_branch_and_a_running_branch_in_one_worktree_are_a_fault() {
    let db = db();
    let a = item(&db, vec![]);
    let b = item(&db, vec![]);
    crate::lists::kept_work::keep(&db, a, &[kept("/d/bae-worktrees/side-1", "work")]).unwrap();
    start(&db, &[b], &[running("/d/bae-worktrees/side-1", "other")]).unwrap();
    let found = faults_of(&db);
    assert!(
        found
            .iter()
            .any(|fault| fault.contains("/d/bae-worktrees/side-1 holds two branches of work")),
        "{found:?}"
    );
}

/// Dropped work belongs to an item closed as dropped.
#[test]
fn dropped_work_on_an_item_not_closed_as_dropped_is_a_fault() {
    use crate::lists::dropped::{DroppedWork, record};
    let db = db();
    let id = item(&db, vec![]);
    let work = DroppedWork {
        repo: "bae".into(),
        worktree: "/d/bae-worktrees/side-1".into(),
        branch: "work".into(),
        commits: Vec::new(),
        files: vec!["loose".into()],
    };
    record(&db, id, std::slice::from_ref(&work)).unwrap();
    assert_eq!(
        faults_of(&db),
        [format!(
            "item {id} records dropped work but is not closed as dropped"
        )]
    );
    close(&db, id, How::Dropped("no".into()), &stamp(NOON)).unwrap();
    assert_eq!(faults_of(&db), Vec::<String>::new());
}

#[test]
fn a_landed_closing_or_a_landed_part_with_no_commit_is_a_fault() {
    let db = db();
    let a = item(&db, vec![]);
    let b = item(&db, vec![]);
    add_part(
        &db,
        a,
        LandedPart {
            repo: "bae".into(),
            commits: vec![],
            at: 1,
        },
    )
    .unwrap();
    close(&db, b, How::Landed(vec![]), &stamp(NOON)).unwrap();
    let found = faults_of(&db);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .any(|f| f == &format!("closed item {b} landed with no commits")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|f| f == &format!("item {a} has a landed part of repo bae with no commits")),
        "{found:?}"
    );
}

#[test]
fn a_project_row_that_names_another_folders_project_is_a_fault() {
    let db = db();
    assert_eq!(faults(&db, "demo").unwrap(), Vec::<String>::new());
    assert_eq!(
        faults(&db, "other").unwrap(),
        ["the project row names demo, and the folder is other's"]
    );
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
    assert!(
        loaded.contains("fail their check")
            && loaded.contains("in a circle")
            && loaded.contains("blocks no open item"),
        "{loaded}"
    );
    let written = store
        .write(&stamp(NOON), "w", |_| Ok(()))
        .unwrap_err()
        .text();
    assert!(
        written.contains("in a circle") && written.contains("blocks no open item"),
        "{written}"
    );
    assert_eq!(
        crate::lists::fixtures::state_of(&store),
        before,
        "nothing repaired it"
    );
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
    assert!(
        refused.contains("is closed and still runs somewhere"),
        "{refused}"
    );
    assert_eq!(crate::lists::fixtures::state_of(&store), before);
}

#[test]
fn a_run_that_ended_with_its_closing_ended_when_its_item_closed() {
    let db = db();
    let id = item(&db, vec![]);
    start(&db, &[id], &[running("/d/bae-worktrees/side-1", "work")]).unwrap();
    crate::lists::running::end(&db, id, Ended::Closed, &stamp(NOON + 5)).unwrap();
    assert_eq!(
        faults_of(&db),
        [format!(
            "item {id} has a run that ended with its closing at {}, but the item is open",
            NOON + 5
        )]
    );
    close(&db, id, How::Landed(vec![sha('a')]), &stamp(NOON + 6)).unwrap();
    assert_eq!(
        faults_of(&db),
        [format!(
            "item {id} has a run that ended with its closing at {}, but the item closed at {}",
            NOON + 5,
            NOON + 6
        )]
    );
}
