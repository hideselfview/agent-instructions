use super::*;
use crate::lists::fixtures::{NOON, item, make_store, stamp};
use crate::testing::Scratch;

fn run(items: Vec<Id>) -> LandRun {
    LandRun {
        project: "demo".into(),
        started_at: 1_790_000_000,
        repo: "bae".into(),
        worktree: "/d/bae-worktrees/side-2".into(),
        branch: Some("work".into()),
        by: "agent A".into(),
        ms: 9_000,
        landed: true,
        items,
        commits: vec![("a".repeat(40), 1_789_999_000)],
        steps: vec![
            Step {
                kind: StepKind::Lock,
                started_at: 1_790_000_000,
                ms: 0,
            },
            Step {
                kind: StepKind::Push,
                started_at: 1_790_000_005,
                ms: 700,
            },
        ],
    }
}

#[test]
fn a_run_is_written_with_its_items_commits_and_steps_in_order() {
    let scratch = Scratch::new("land-run-record");
    let store = make_store(&scratch.places(), &["bae"]);
    let id = store
        .write(&stamp(NOON), "add item", |db| Ok(item(db, vec![])))
        .unwrap();
    record(&store.database(), &run(vec![id, id])).unwrap();
    let db = store.open_existing().unwrap();
    let row: (i64, String, Option<String>, i64, i64) = db
        .query_row(
            "SELECT started_at, repo, branch, ms, landed FROM land_run",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        row,
        (1_790_000_000, "bae".into(), Some("work".into()), 9_000, 1)
    );
    let items: Vec<i64> = db
        .prepare("SELECT item_id FROM land_run_item")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(
        items,
        vec![i64::from(id.0)],
        "an item named twice counts once"
    );
    let steps: Vec<(String, i64)> = db
        .prepare("SELECT kind, ms FROM land_step ORDER BY position")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(
        steps,
        vec![("lock".to_string(), 0), ("push".to_string(), 700)]
    );
    let committed: i64 = db
        .query_row("SELECT committed_at FROM land_run_commit", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(committed, 1_789_999_000);
}

#[test]
fn a_run_with_a_commit_that_is_no_sha_is_refused_and_writes_nothing() {
    let scratch = Scratch::new("land-run-refused");
    let store = make_store(&scratch.places(), &["bae"]);
    let mut bad = run(vec![]);
    bad.commits = vec![("not a sha".into(), 1)];
    assert!(record(&store.database(), &bad).is_err());
    let db = store.open_existing().unwrap();
    let runs: i64 = db
        .query_row("SELECT count(*) FROM land_run", [], |row| row.get(0))
        .unwrap();
    assert_eq!(runs, 0, "the run's rows commit whole or not at all");
}
