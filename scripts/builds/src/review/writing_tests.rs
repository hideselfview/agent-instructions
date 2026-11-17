use super::*;
use crate::lists::item_fixtures::demo;
use crate::review::fakes::fake;

#[test]
fn the_writing_check_prints_the_writing_groups_findings_and_records_nothing() {
    let demo = demo("review-writing");
    let fake = fake(&demo.scratch.dir);
    std::fs::write(
        fake.root
            .join("rules/beta")
            .join(format!("{WRITING_RULE}.md")),
        "---\ndigest: d\npaths:\n  - '**/*.rs'\nblocking: true\n---\n\nWrite plain sentences.\n",
    )
    .unwrap();
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
    let checked = check(&side, "main", &fake.root, "demo", &fake.program).unwrap();
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert_eq!(prompts[0].0, "beta");
    assert!(
        checked.text.contains("beta (opus): 1 finding")
            && checked.text.contains("The sentence is not plain."),
        "{}",
        checked.text
    );
    assert!(!checked.clean);
    let conn = demo.store.open_existing().unwrap();
    for table in [
        "review_landing",
        "review_run",
        "review_call",
        "review_finding",
    ] {
        let rows: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
}
