use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{Fake, fake};

fn writing_rule(fake: &Fake) {
    std::fs::write(
        fake.root
            .join("rules/beta")
            .join(format!("{WRITING_RULE}.md")),
        "---\ndigest: d\npaths:\n  - '**/*.rs'\nblocking: true\n---\n\nWrite plain sentences.\n",
    )
    .unwrap();
}

fn stamp(at: i64) -> Stamp {
    Stamp {
        at,
        by: "tester".to_string(),
    }
}

fn check_on(demo: &Demo, fake: &Fake, side: &Path) -> Result<Checked> {
    check(&Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: side,
        base: "main",
        key: "branch:work",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(NOON),
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
fn the_writing_check_prints_the_writing_groups_findings_and_records_only_its_run() {
    let demo = demo("review-writing");
    let fake = fake(&demo.scratch.dir);
    writing_rule(&fake);
    fake.answer(
        "beta",
        &[
            ("rule-b", None),
            (
                WRITING_RULE,
                Some((
                    "Write plain sentences.",
                    "a.rs",
                    1,
                    "a.rs",
                    "The sentence is not plain.",
                )),
            ),
        ],
    );
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let checked = check_on(&demo, &fake, &side).unwrap();
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert_eq!(prompts[0].0, "beta");
    assert!(
        checked.text.contains("beta (opus): 1 finding")
            && checked.text.contains("The sentence is not plain.")
            && checked.text.contains("writing check 1 of 2 on branch:work"),
        "{}",
        checked.text
    );
    assert!(!checked.clean);
    for table in [
        "review_landing",
        "review_run",
        "review_call",
        "review_finding",
    ] {
        assert_eq!(count(&demo, table), 0, "{table}");
    }
    assert_eq!(count(&demo, "review_writing_run"), 1);
}

#[test]
fn the_writing_check_refuses_a_third_run_until_the_branch_has_another_review() {
    let demo = demo("review-writing-cap");
    let fake = fake(&demo.scratch.dir);
    writing_rule(&fake);
    fake.answer("beta", &[("rule-b", None), (WRITING_RULE, None)]);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.fail("beta");
    let failed = check_on(&demo, &fake, &side).unwrap();
    assert!(!failed.clean && failed.text.contains("the review failed"));
    std::fs::remove_file(fake.dir.join("fail-beta")).unwrap();
    assert!(check_on(&demo, &fake, &side).unwrap().clean);
    let second = check_on(&demo, &fake, &side).unwrap();
    assert!(
        second
            .text
            .contains("writing check 2 of 2 on branch:work since its last review"),
        "{}",
        second.text
    );
    fake.prompts();
    let refused = check_on(&demo, &fake, &side).unwrap_err().text();
    assert!(
        refused.contains("fix what the second run found, and report"),
        "{refused}"
    );
    assert!(fake.prompts().is_empty());
    demo.store
        .write(&stamp(NOON + 1), "review", |db| {
            let landing = record::add_landing(db, "branch:work", "demo", NOON + 1)?;
            record::add_run(
                db,
                &record::NewRun {
                    landing: Some(landing),
                    kind: record::Kind::Review { changed_lines: 1 },
                    label: None,
                    at: NOON + 1,
                    base: "b",
                    head: "h",
                    since: None,
                    grouping: "g",
                    rules_version: "",
                },
                &[],
            )
        })
        .unwrap();
    let after = check_on(&demo, &fake, &side).unwrap();
    assert!(
        after.text.contains("writing check 1 of 2 on branch:work"),
        "{}",
        after.text
    );
}
