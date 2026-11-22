use super::*;
use crate::db;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::demo;
use crate::review::fakes::{Fake, Raised, fake};
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
        closed_by: None,
    }
}

#[test]
fn five_percent_of_landings_are_audited_unless_a_group_they_ran_missed_more() {
    assert_eq!(chance(false), BASE_CHANCE);
    assert_eq!(chance(true), RISEN_CHANCE);
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

fn group_on(name: &str, model: &str) -> Group {
    Group::parse(
        name,
        &format!("name = \"{name}\"\nquestion = \"q\"\ntier = \"{model}\"\n"),
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn the_shuffle_is_the_same_for_a_seed_and_holds_every_rule_once_on_its_groups_model() {
    let rules: Vec<Rule> = (0..7)
        .map(|n| Rule::parse(&format!("r{n}"), "---\npaths: ['**']\n---\nb\n"))
        .collect();
    let deep = group_on("deep", "high");
    let fast = group_on("fast", "medium");
    let audited: Vec<(&Group, Vec<&Rule>)> = vec![
        (&deep, rules[..5].iter().collect()),
        (&fast, rules[5..].iter().collect()),
    ];
    let a = shuffled_groups(&audited, 3, &mut Rng(42));
    let b = shuffled_groups(&audited, 3, &mut Rng(42));
    assert_eq!(a, b);
    assert_eq!(a.len(), 3);
    let mut all: Vec<String> = a.iter().flat_map(|g| g.rules.clone()).collect();
    all.sort();
    assert_eq!(all, ["r0", "r1", "r2", "r3", "r4", "r5", "r6"]);
    for shuffled in &a {
        let model = if shuffled.rules.iter().any(|r| r == "r5" || r == "r6") {
            "medium"
        } else {
            "high"
        };
        assert_eq!(shuffled.model.to_string(), model, "{shuffled:?}");
        assert!(
            shuffled
                .rules
                .iter()
                .all(|r| (r == "r5" || r == "r6") == (model == "medium")),
            "{shuffled:?}"
        );
    }
}

fn stamp() -> crate::lists::Stamp {
    crate::lists::Stamp {
        at: NOON,
        by: "tester".to_string(),
    }
}

/// A reviewed change of a.rs, landed with no finding; returns the landing.
fn landed(demo: &crate::lists::item_fixtures::Demo, fake: &Fake, side: &Path) -> i64 {
    demo.commit(side, "a.rs");
    let reviewed = review(&ReviewInputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: side,
        base: "main",
        head: None,
        key: "k",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        brief: None,
        runner: None,
    })
    .unwrap();
    assert!(reviewed.refusal.is_none());
    let landing = reviewed.landing.unwrap();
    land(demo, landing, &stamp());
    fake.prompts();
    landing
}

fn audit_of(
    demo: &crate::lists::item_fixtures::Demo,
    fake: &Fake,
    side: &Path,
    landing: i64,
) -> String {
    audit(&Inputs {
        store: &demo.store,
        project: "demo",
        repo_dir: side,
        landing,
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        seed: 7,
    })
    .unwrap()
}

#[test]
fn an_audit_records_two_runs_names_the_misses_and_feeds_the_miss_rate() {
    let demo = demo("review-audit");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let landing = landed(&demo, &fake, &side);
    // The audits raise a finding the review did not, and its check fails.
    fake.answer("audit rule-a", &[("rule-a", Some(BAD))]);
    fake.answer("shuffle 1", &[("rule-a", None)]);
    fake.fail("model-check rule-a");
    let text = audit_of(&demo, &fake, &side, landing);
    assert!(
        text.contains("1 candidate misses the review did not raise"),
        "{text}"
    );
    assert!(text.contains("the check gave no answer"), "{text}");
    let prompts = fake.prompts();
    assert_eq!(
        prompts
            .iter()
            .filter(|(g, _)| g.starts_with("audit") || g.starts_with("shuffle"))
            .count(),
        2,
        "one reviewer per rule, and one shuffled group"
    );
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let group_of_rule = |rule: &str| (rule == "rule-a").then(|| "alpha".to_string());
    assert_eq!(rates(&conn, &group_of_rule).unwrap()[0].misses, 0);
    let open: Vec<i64> = open_misses(&conn)
        .unwrap()
        .iter()
        .map(|(at, _)| *at)
        .collect();
    assert_eq!(open, [landing]);
    drop(conn);
    // The check of the open misses settles the one the audit left.
    std::fs::remove_file(fake.dir.join("fail-model-check rule-a")).unwrap();
    fake.answer_task(
        "model-check rule-a",
        serde_json::json!({"verdict": "confirmed", "why": "a.rs does the bad thing."}),
    );
    let (text, failed) = check_open_misses(&OpenMisses {
        store: &demo.store,
        project: "demo",
        root: &fake.root,
        reviewer: &fake.program,
        model: &crate::review::Selection::Named("opus".to_string()),
        stamp: &stamp(),
    })
    .unwrap();
    assert!(!failed, "{text}");
    assert!(text.contains("confirmed"), "{text}");
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let findings = record::findings_of_landing(&conn, landing).unwrap();
    let miss = candidate_misses(&findings)[0];
    assert_eq!(
        miss.settled.as_ref().unwrap().last().why,
        "the opus check confirmed it: a.rs does the bad thing."
    );
    let rate = &rates(&conn, &group_of_rule).unwrap()[0];
    assert_eq!(
        (rate.group.as_str(), rate.audits, rate.misses),
        ("alpha", 1, 1)
    );
    // No group's misses rose, so the landing is audited at the base chance.
    assert!(sampled(&conn, landing, &group_of_rule, BASE_CHANCE - 0.01).unwrap());
    assert!(!sampled(&conn, landing, &group_of_rule, BASE_CHANCE).unwrap());
}

#[test]
fn an_audit_asks_only_the_rules_of_the_groups_the_review_ran() {
    let demo = demo("review-audit-ran");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let landing = landed(&demo, &fake, &side);
    // After the landing, rule-b of group beta covers the changed a.rs too,
    // but beta did not run on the landing, so the audit does not ask it.
    crate::review::fakes::rule_file(
        &fake.root,
        "beta",
        "rule-b",
        "**/*.rs",
        false,
        "**Rule B.** Never do the other thing.",
    );
    fake.answer("audit rule-a", &[("rule-a", None)]);
    fake.answer("shuffle 1", &[("rule-a", None)]);
    let text = audit_of(&demo, &fake, &side, landing);
    assert!(text.contains("0 candidate misses"), "{text}");
    let asked: Vec<String> = fake.prompts().into_iter().map(|(g, _)| g).collect();
    assert_eq!(asked, ["audit rule-a", "shuffle 1"]);
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let calls = record::calls_of_landing(&conn, landing).unwrap();
    let shuffled = calls.iter().find(|c| c.group == "shuffle 1").unwrap();
    assert_eq!(shuffled.rules, ["rule-a"]);
    assert_eq!(
        shuffled.model, "sonnet",
        "alpha, the group of rule-a, runs sonnet"
    );
}

#[test]
fn an_audit_drops_a_miss_the_check_rejects_with_the_models_reason() {
    let demo = demo("review-audit-rejected");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let landing = landed(&demo, &fake, &side);
    fake.answer("audit rule-a", &[("rule-a", Some(BAD))]);
    fake.answer("shuffle 1", &[("rule-a", None)]);
    fake.answer_task(
        "model-check rule-a",
        serde_json::json!({"verdict": "rejected", "why": "The clause does not cover a.rs."}),
    );
    let text = audit_of(&demo, &fake, &side, landing);
    assert!(text.contains("dropped"), "{text}");
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let findings = record::findings_of_landing(&conn, landing).unwrap();
    let settled = candidate_misses(&findings)[0].settled.clone().unwrap();
    assert_eq!(settled.last().how, "dropped");
    assert_eq!(
        settled.last().why,
        "the opus check rejected it: The clause does not cover a.rs."
    );
}

#[test]
fn an_audit_names_a_group_the_review_ran_that_the_rules_checkout_no_longer_holds() {
    let demo = demo("review-audit-renamed");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let landing = landed(&demo, &fake, &side);
    crate::review::fakes::group_file(&fake.root, "alpha", "renamed", "medium", "");
    let text = audit_of(&demo, &fake, &side, landing);
    assert!(
        text.contains("not audited: group alpha ran on the landing's review, and the rules checkout has no group of that name"),
        "{text}"
    );
    assert!(
        text.contains("no group the landing's review ran is left to audit"),
        "{text}"
    );
    assert!(fake.prompts().is_empty());
}

/// Records <landing> as landed with its newest run, as `builds review landed`
/// does after the push.
fn land(demo: &crate::lists::item_fixtures::Demo, landing: i64, stamp: &crate::lists::Stamp) {
    demo.store
        .write(stamp, "landed", |db| {
            let run = record::runs_of_landing(db, landing)?.last().unwrap().id;
            record::record_landed(db, landing, run, stamp.at)
        })
        .unwrap();
}

#[test]
fn a_landing_whose_change_has_not_landed_is_not_audited() {
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
        runner: None,
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
    assert!(refused.contains("has not landed"), "{refused}");
}

#[test]
fn an_open_miss_is_checked_although_another_landing_raised_its_rule_and_file() {
    let demo = demo("review-audit-other-landing");
    let fake = fake(&demo.scratch.dir);
    let first = demo.add_side(1, "agent", "first");
    demo.commit(&first, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let raised = review(&ReviewInputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: &first,
        base: "main",
        head: None,
        key: "first",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        brief: None,
        runner: None,
    })
    .unwrap()
    .landing
    .unwrap();
    land(&demo, raised, &stamp());
    fake.answer("alpha", &[("rule-a", None)]);
    let second = demo.add_side(2, "agent", "second");
    let landing = landed(&demo, &fake, &second);
    fake.answer("audit rule-a", &[("rule-a", Some(BAD))]);
    fake.fail("model-check rule-a");
    audit_of(&demo, &fake, &second, landing);
    std::fs::remove_file(fake.dir.join("fail-model-check rule-a")).unwrap();
    fake.answer_task(
        "model-check rule-a",
        serde_json::json!({"verdict": "confirmed", "why": "w"}),
    );
    let (text, _) = check_open_misses(&OpenMisses {
        store: &demo.store,
        project: "demo",
        root: &fake.root,
        reviewer: &fake.program,
        model: &crate::review::Selection::Named("opus".to_string()),
        stamp: &stamp(),
    })
    .unwrap();
    assert!(text.contains("settled confirmed"), "{text}");
}
