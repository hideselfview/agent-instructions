use super::*;
use crate::lists::Stamp;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::cases;
use crate::review::fakes::{Fake, fake};
use crate::review::{Inputs as ReviewInputs, review};
use crate::testing::git as run_git;

const CLEAN: &str = "let total = rows.iter().map(|row| row.bytes).sum();";

fn stamp() -> Stamp {
    Stamp {
        at: NOON,
        by: "tester".to_string(),
    }
}

/// A landed change of a.rs and b.swift. The reviewers raise one finding in
/// each; the coordinator confirms the rule-a one with its own reason and the
/// rule-b one with <rule_b_why>. Returns the landing.
fn landed(demo: &Demo, fake: &Fake, side: &Path, rule_b_why: &str) -> i64 {
    std::fs::write(side.join("a.rs"), format!("bad();\n{CLEAN}\n}}\n")).unwrap();
    std::fs::write(side.join("b.swift"), "other()\n").unwrap();
    run_git(side, &["add", "a.rs", "b.swift"]);
    run_git(side, &["commit", "-q", "-m", "Add a and b"]);
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some(("Never do the bad thing here.", "a.rs", 1, "bad();", "p")),
        )],
    );
    fake.answer(
        "beta",
        &[(
            "rule-b",
            Some(("Never do the other thing.", "b.swift", 1, "other()", "p")),
        )],
    );
    let landing = review(&ReviewInputs {
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
    .unwrap()
    .landing
    .unwrap();
    demo.store
        .write(&stamp(), "settle and land", |db| {
            let findings = record::findings_of_landing(db, landing)?;
            let id_of = |rule: &str| findings.iter().find(|f| f.rule == rule).unwrap().id;
            record::settle(
                db,
                id_of("rule-a"),
                "confirmed",
                "bad() runs the bad thing on every row",
                NOON,
                "tester",
            )?;
            record::settle(db, id_of("rule-b"), "confirmed", rule_b_why, NOON, "tester")?;
            let run = record::runs_of_landing(db, landing)?.last().unwrap().id;
            record::record_landed(db, landing, run, NOON)
        })
        .unwrap();
    fake.prompts();
    landing
}

fn propose_in(fake: &Fake, side: &Path, store: &Store) -> Proposed {
    propose(&Inputs {
        store,
        project: "demo",
        repo_dir: side,
        root: &fake.root,
        reviewer: &fake.program,
        model: "opus",
        per_group: 2,
    })
    .unwrap()
}

#[test]
fn a_landing_becomes_a_case_of_its_specific_findings_the_check_confirms_and_its_clean_lines() {
    let demo = demo("review-propose");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let landing = landed(&demo, &fake, &side, GENERIC[0]);
    fake.answer_task(
        "model-check rule-a",
        serde_json::json!({"verdict": "confirmed", "why": "bad() is the bad thing."}),
    );
    let proposed = propose_in(&fake, &side, &demo.store);
    assert!(!proposed.failed, "{}", proposed.text);
    let prompts = fake.prompts();
    assert_eq!(
        prompts
            .iter()
            .map(|(task, _)| task.as_str())
            .collect::<Vec<_>>(),
        ["model-check rule-a"],
        "propose does not check a finding settled with a generic reason"
    );
    let all = cases::load(&fake.root).unwrap();
    assert_eq!(all.len(), 1, "{}", proposed.text);
    let case = &all[0];
    assert!(case.name.starts_with(&format!("landing-{landing}-")));
    assert_eq!(
        case.expected,
        [cases::Expect {
            rule: "rule-a".to_string(),
            file: "a.rs".to_string()
        }]
    );
    let verdicts: Vec<(&str, &str)> = case
        .labels
        .iter()
        .map(|l| (l.code.as_str(), l.verdict.as_str()))
        .collect();
    assert_eq!(verdicts, [("bad();", "finding"), (CLEAN, "none")]);
    assert_eq!(
        crate::review::files::paths(&case.patch),
        ["a.rs"],
        "the case holds only the files of its confirmed findings"
    );
    // A second run keeps the case and asks no model again.
    let again = propose_in(&fake, &side, &demo.store);
    assert!(again.text.contains("has a case already"), "{}", again.text);
    assert!(fake.prompts().is_empty());
}

#[test]
fn a_landing_whose_findings_the_check_rejects_becomes_no_case() {
    let demo = demo("review-propose-rejected");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    landed(&demo, &fake, &side, "other() repeats the call above");
    fake.answer_task(
        "model-check rule-a",
        serde_json::json!({"verdict": "rejected", "why": "The clause does not cover bad()."}),
    );
    fake.answer_task(
        "model-check rule-b",
        serde_json::json!({"verdict": "rejected", "why": "other() follows the rule."}),
    );
    let proposed = propose_in(&fake, &side, &demo.store);
    assert!(
        proposed
            .text
            .contains("rejected: The clause does not cover bad()."),
        "{}",
        proposed.text
    );
    assert!(cases::load(&fake.root).unwrap().is_empty());
}

#[test]
fn a_confirmed_miss_becomes_a_case_that_expects_it() {
    let demo = demo("review-propose-miss");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let landing = landed(&demo, &fake, &side, GENERIC[0]);
    fake.answer_task(
        "model-check rule-a",
        serde_json::json!({"verdict": "confirmed", "why": "w"}),
    );
    // An audit raises a rule-a finding in b.swift that the review did not
    // raise, and the check confirms it.
    fake.answer(
        "audit rule-a",
        &[(
            "rule-a",
            Some(("Never do the bad thing here.", "b.swift", 1, "other()", "p")),
        )],
    );
    fake.answer("audit rule-b", &[("rule-b", None)]);
    let audited = crate::review::audit::audit(&crate::review::audit::Inputs {
        store: &demo.store,
        project: "demo",
        repo_dir: &side,
        landing,
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        seed: 7,
    })
    .unwrap();
    assert!(audited.contains("settled confirmed"), "{audited}");
    let proposed = propose_in(&fake, &side, &demo.store);
    assert!(!proposed.failed, "{}", proposed.text);
    let expected: Vec<(String, String)> = cases::load(&fake.root)
        .unwrap()
        .iter()
        .flat_map(|c| c.expected.iter())
        .map(|e| (e.rule.clone(), e.file.clone()))
        .collect();
    assert!(
        expected.contains(&("rule-a".to_string(), "b.swift".to_string())),
        "{expected:?}"
    );
    let miss_case = cases::load(&fake.root)
        .unwrap()
        .into_iter()
        .find(|c| c.expected.iter().any(|e| e.file == "b.swift"))
        .unwrap();
    assert_eq!(crate::review::files::paths(&miss_case.patch), ["b.swift"]);
}

#[test]
fn a_landing_whose_commits_the_repo_lost_gives_no_case_and_says_why() {
    let demo = demo("review-propose-lost");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    landed(&demo, &fake, &side, GENERIC[0]);
    run_git(&side, &["reset", "-q", "--hard", "main"]);
    run_git(&side, &["reflog", "expire", "--expire=now", "--all"]);
    run_git(&side, &["gc", "-q", "--prune=now"]);
    let proposed = propose_in(&fake, &side, &demo.store);
    assert!(
        proposed.text.contains(
            "landing 1: propose wrote no case, since the repo does not have the commits of run"
        ),
        "{}",
        proposed.text
    );
    assert!(fake.prompts().is_empty());
}

#[test]
fn a_finding_whose_code_is_not_in_the_change_is_named_and_left_out() {
    let demo = demo("review-propose-code-gone");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    landed(&demo, &fake, &side, "other() repeats the call above");
    let conn = demo.store.open_existing().unwrap();
    let mut findings = record::findings_of_landing(&conn, 1).unwrap();
    drop(conn);
    let b = findings.iter_mut().find(|f| f.rule == "rule-b").unwrap();
    b.code = "not in it".to_string();
    let change = run::diff_between(&side, "main", "HEAD").unwrap();
    let patch = cases::patch_of(&side, &change.base, &change.head).unwrap();
    assert_eq!(
        path_in(&change.diff, &patch, b),
        Err("its code is not in the diff of b.swift".to_string())
    );
}
