use super::*;
use crate::db;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::demo;
use crate::review::fakes::{Raised, fake};
use crate::review::record::Finding;
use crate::review::{Inputs as ReviewInputs, review};

const BAD: Raised = (
    "Never do the bad thing here.",
    "a.rs",
    1,
    "a.rs",
    "The file does the bad thing.",
);

fn finding(kind: &str, rule: &str, file: &str, check_failed: Option<&str>) -> Finding {
    Finding {
        id: 1,
        run_id: 1,
        kind: kind.to_string(),
        landing_id: Some(1),
        group: "g".to_string(),
        rule: rule.to_string(),
        clause: "c".to_string(),
        file: file.to_string(),
        line: 1,
        code: "x".to_string(),
        problem: "p".to_string(),
        blocking: true,
        check_failed: check_failed.map(str::to_string),
        settled: None,
    }
}

#[test]
fn every_landing_is_audited_under_a_new_grouping_and_five_percent_after_unless_a_group_missed_more()
{
    assert_eq!(chance(1, false), 1.0);
    assert_eq!(chance(FULL_FOR, false), 1.0);
    assert_eq!(chance(FULL_FOR + 1, false), BASE_CHANCE);
    assert_eq!(chance(FULL_FOR + 1, true), RISEN_CHANCE);
}

#[test]
fn a_miss_is_an_audit_finding_the_review_did_not_raise_in_that_rule_and_file() {
    let findings = vec![
        finding("review", "rule-a", "a.rs", None),
        finding("audit", "rule-a", "a.rs", None),
        finding("audit", "rule-a", "b.rs", None),
        finding("audit", "rule-b", "a.rs", None),
        finding(
            "audit",
            "rule-c",
            "c.rs",
            Some("its code is not in the diff of c.rs"),
        ),
        finding("review", "rule-d", "d.rs", Some("dropped")),
        finding("audit", "rule-d", "d.rs", None),
    ];
    let misses: Vec<(&str, &str)> = candidate_misses(&findings)
        .iter()
        .map(|f| (f.rule.as_str(), f.file.as_str()))
        .collect();
    assert_eq!(
        misses,
        [("rule-a", "b.rs"), ("rule-b", "a.rs"), ("rule-d", "d.rs")]
    );
}

#[test]
fn a_groups_misses_have_risen_when_its_recent_audits_missed_more_than_its_earlier_ones() {
    let rate = |audits, misses, recent_audits, recent_misses| Rate {
        group: "g".to_string(),
        audits,
        misses,
        recent_misses,
        recent_audits,
    };
    assert!(rate(20, 2, 10, 2).risen());
    assert!(!rate(20, 4, 10, 2).risen());
    assert!(!rate(10, 1, 10, 1).risen());
    assert!(!rate(20, 2, 10, 0).risen());
}

#[test]
fn the_shuffle_is_the_same_for_a_seed_and_holds_every_rule_once() {
    let rules: Vec<Rule> = (0..7)
        .map(|n| Rule::parse(&format!("r{n}"), "---\npaths: ['**']\n---\nb\n"))
        .collect();
    let refs: Vec<&Rule> = rules.iter().collect();
    let groups = Groups::parse(
        "[[group]]\nname = \"g\"\nquestion = \"q\"\nmodel = \"sonnet\"\nrules = [\"r0\"]\n",
    )
    .unwrap();
    let a = shuffled_groups(&groups, &refs, 3, &mut Rng(42));
    let b = shuffled_groups(&groups, &refs, 3, &mut Rng(42));
    assert_eq!(a, b);
    assert_eq!(a.len(), 3);
    let mut all: Vec<String> = a.iter().flat_map(|g| g.rules.clone()).collect();
    all.sort();
    assert_eq!(all, ["r0", "r1", "r2", "r3", "r4", "r5", "r6"]);
}

#[test]
fn an_audit_records_two_runs_names_the_misses_and_feeds_the_miss_rate() {
    let demo = demo("review-audit");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let stamp = crate::lists::Stamp {
        at: NOON,
        by: "tester".to_string(),
    };
    let reviewed = review(&ReviewInputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: &side,
        base: "main",
        head: None,
        key: "k",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp,
        brief: None,
    })
    .unwrap();
    assert!(reviewed.refusal.is_none());
    let landing = reviewed.landing.unwrap();
    fake.prompts();
    // The audits raise a finding the review did not.
    fake.answer("audit rule-a", &[("rule-a", Some(BAD))]);
    fake.answer("shuffle 1", &[("rule-a", None)]);
    let text = audit(&Inputs {
        store: &demo.store,
        project: "demo",
        repo_dir: &side,
        landing,
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp,
        seed: 7,
    })
    .unwrap();
    assert!(
        text.contains("1 candidate misses the review did not raise"),
        "{text}"
    );
    assert!(text.contains("builds review settle"));
    let prompts = fake.prompts();
    assert_eq!(
        prompts.len(),
        2,
        "one reviewer per rule, and one shuffled group"
    );
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let findings = record::findings_of_landing(&conn, landing).unwrap();
    let miss = candidate_misses(&findings)[0].id;
    let group_of_rule = |rule: &str| (rule == "rule-a").then(|| "alpha".to_string());
    // Unsettled, it is no miss yet.
    assert_eq!(rates(&conn, &group_of_rule).unwrap()[0].misses, 0);
    drop(conn);
    demo.store
        .write(&stamp, "settle", |db| {
            record::settle(db, miss, "confirmed", "a real miss", NOON, "tester")
        })
        .unwrap();
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let rate = &rates(&conn, &group_of_rule).unwrap()[0];
    assert_eq!(
        (rate.group.as_str(), rate.audits, rate.misses),
        ("alpha", 1, 1)
    );
    // Under a grouping with one landing, every landing is audited.
    assert!(sampled(&conn, landing, &group_of_rule, 0.99).unwrap());
}

#[test]
fn a_landing_whose_review_did_not_pass_is_not_audited() {
    let demo = demo("review-audit-open");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let stamp = crate::lists::Stamp {
        at: NOON,
        by: "tester".to_string(),
    };
    let reviewed = review(&ReviewInputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: &side,
        base: "main",
        head: None,
        key: "k",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp,
        brief: None,
    })
    .unwrap();
    assert!(reviewed.refusal.is_some());
    let refused = audit(&Inputs {
        store: &demo.store,
        project: "demo",
        repo_dir: &side,
        landing: reviewed.landing.unwrap(),
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp,
        seed: 1,
    })
    .unwrap_err()
    .text();
    assert!(refused.contains("no passed review"), "{refused}");
}
