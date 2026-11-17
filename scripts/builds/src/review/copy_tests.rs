use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{Fake, copy_group, fake};

fn stamp(at: i64) -> Stamp {
    Stamp {
        at,
        by: "tester".to_string(),
    }
}

fn copy_on(demo: &Demo, fake: &Fake, side: &Path) -> Checked {
    check(&Inputs {
        store: &demo.store,
        project: "demo",
        worktree: side,
        base: "main",
        label: "branch:work",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(NOON),
    })
    .unwrap()
}

/// A finding of rule-c on <code> in a.rs.
fn site(code: &str) -> serde_json::Value {
    serde_json::json!({
        "file": "a.rs", "line": 1, "kind": "string", "code": code, "verdict": "finding",
        "rule": "rule-c", "clause": "Write plain sentences.", "problem": "The sentence is not plain."
    })
}

fn count(demo: &Demo, table: &str) -> i64 {
    let conn = demo.store.open_existing().unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn the_copy_review_asks_only_the_copy_groups_and_records_its_findings_each_time() {
    let demo = demo("review-copy");
    let fake = fake(&demo.scratch.dir);
    copy_group(&fake, serde_json::json!([site("a.rs")]));
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    for time in 1..=3 {
        let checked = copy_on(&demo, &fake, &side);
        let prompts = fake.prompts();
        let groups: Vec<&str> = prompts.iter().map(|(group, _)| group.as_str()).collect();
        assert_eq!(groups, ["copy"], "{}", checked.text);
        assert!(
            checked.text.contains("copy (opus): 1 finding")
                && checked.text.contains("The sentence is not plain."),
            "{}",
            checked.text
        );
        assert!(!checked.clean);
        assert_eq!(count(&demo, "review_run"), time, "the store keeps each run");
    }
    let conn = demo.store.open_existing().unwrap();
    let run = record::findings_of_run(&conn, 1).unwrap();
    assert_eq!(run.len(), 1);
    assert_eq!((run[0].kind.as_str(), run[0].landing_id), ("copy", None));
    assert_eq!(count(&demo, "review_landing"), 0);
}

#[test]
fn a_review_asks_no_copy_group() {
    let demo = demo("review-no-copy");
    let fake = fake(&demo.scratch.dir);
    copy_group(&fake, serde_json::json!([site("a.rs")]));
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let reported = crate::review::review(&crate::review::Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: &side,
        base: "main",
        head: None,
        key: "branch:work",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(NOON),
        brief: None,
    })
    .unwrap();
    let groups: Vec<String> = fake.prompts().into_iter().map(|(group, _)| group).collect();
    assert_eq!(groups, ["alpha"], "{}", reported.text);
    assert!(reported.refusal.is_none(), "{:?}", reported.refusal);
}
