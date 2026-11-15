use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{Fake, Raised, fake};

fn stamp() -> Stamp {
    Stamp {
        at: NOON,
        by: "tester".to_string(),
    }
}

fn run_review(demo: &Demo, fake: &Fake, side: &Path, key: &str) -> Reported {
    try_review(demo, fake, side, key).unwrap()
}

fn try_review(demo: &Demo, fake: &Fake, side: &Path, key: &str) -> Result<Reported> {
    review(&Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: side,
        base: "main",
        head: None,
        key,
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        brief: None,
    })
}

const BAD: Raised = (
    "Never do the bad thing here.",
    "a.rs",
    1,
    "a.rs",
    "The file does the bad thing.",
);

fn settle_as(demo: &Demo, finding: i64, how: &str, why: &str) {
    let stamp = stamp();
    demo.store
        .write(&stamp, "settle", |db| {
            record::settle(db, finding, how, why, stamp.at, "tester")
        })
        .unwrap();
}

fn findings_of(demo: &Demo, landing: i64) -> Vec<record::Finding> {
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    record::findings_of_landing(&conn, landing).unwrap()
}

#[test]
fn only_the_groups_whose_rules_cover_a_changed_path_run() {
    let demo = demo("review-pick");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let reported = run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert_eq!(prompts[0].0, "alpha");
    assert!(
        reported.text.contains("alpha (sonnet): no findings"),
        "{}",
        reported.text
    );
    assert!(!reported.text.contains("beta"));
    assert!(reported.refusal.is_none());
    // The reviewer read the rule's full text, the project facts and the diff.
    assert!(prompts[0].1.contains("Never do the bad thing here."));
    assert!(prompts[0].1.contains("Demo project facts."));
    assert!(prompts[0].1.contains("+a.rs"));
    assert!(
        reported.text.contains("builds land: review landing")
            && reported.text.contains("takes these commits"),
        "{}",
        reported.text
    );
}

#[test]
fn a_finding_whose_quote_is_not_in_the_diff_is_dropped_and_recorded_as_dropped() {
    let demo = demo("review-quotes");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some((
                "Never do the bad thing here.",
                "a.rs",
                1,
                "code that is not there",
                "p",
            )),
        )],
    );
    let reported = run_review(&demo, &fake, &side, "k");
    assert!(
        reported.text.contains("dropped by builds"),
        "{}",
        reported.text
    );
    assert!(reported.text.contains("alpha (sonnet): no findings"));
    assert!(reported.refusal.is_none());
    let findings = findings_of(&demo, reported.landing.unwrap());
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0]
            .check_failed
            .as_deref()
            .unwrap()
            .contains("not in the diff")
    );
    // A clause that is not in the rule is dropped the same way.
    let side2 = demo.add_side(2, "agent", "work2");
    demo.commit(&side2, "a.rs");
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some(("A clause nobody wrote.", "a.rs", 1, "a.rs", "p")),
        )],
    );
    let reported = run_review(&demo, &fake, &side2, "k2");
    assert!(
        reported
            .text
            .contains("its clause is not in the rule's text"),
        "{}",
        reported.text
    );
    assert!(reported.refusal.is_none());
}

#[test]
fn a_change_gets_one_review_and_a_second_run_is_refused() {
    let demo = demo("review-once");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    demo.commit(&side, "b.swift");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let first = run_review(&demo, &fake, &side, "k");
    assert_eq!(fake.prompts().len(), 2);
    let refusal = first
        .refusal
        .expect("an unsettled finding keeps the commits from landing");
    assert!(refusal.contains("1 finding is not settled"), "{refusal}");
    assert!(
        refusal.contains("builds review settle <finding> confirmed|dropped --why TEXT"),
        "{refusal}"
    );
    let finding = findings_of(&demo, first.landing.unwrap())[0].id;
    settle_as(&demo, finding, "confirmed", "a real break");
    // The same commits: the change has its review, and no reviewer is asked.
    let again = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(fake.prompts().is_empty());
    assert!(
        again.contains(landing::ONE_REVIEW)
            && again.contains("so builds review does not review it again")
            && !again.contains("did not read"),
        "{again}"
    );
    // A fix commit is not reviewed either: the refusal names it as a commit
    // the review did not read, for the coordinator to read and name.
    let fix = demo.commit(&side, "fix.rs");
    let again = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(fake.prompts().is_empty());
    assert!(
        again.contains(&format!("Commits the review did not read: {}", &fix[..10]))
            && again.contains("builds review fixed <finding> <sha>"),
        "{again}"
    );
    let commit = landing::fix_commit(&side, &fix).unwrap();
    let stamp = stamp();
    demo.store
        .write(&stamp, "fixed", |db| {
            record::record_fix(db, finding, &commit, stamp.at, "tester")
        })
        .unwrap();
    // The record holds the one review run.
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    assert_eq!(
        landing::review_runs(&conn, first.landing.unwrap())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_reviewer_that_fails_keeps_the_commits_from_landing_and_only_it_is_asked_again() {
    let demo = demo("review-failed");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    demo.commit(&side, "b.swift");
    fake.fail("alpha");
    let reported = run_review(&demo, &fake, &side, "k");
    assert!(reported.text.contains("boom"), "{}", reported.text);
    let refusal = reported.refusal.expect("a failed review keeps the commits");
    assert!(
        refusal.contains("group alpha covers a path a reviewed commit changes and gave no answer"),
        "{refusal}"
    );
    fake.prompts();
    std::fs::remove_file(fake.dir.join("fail-alpha")).unwrap();
    let again = run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].0, "alpha");
    assert!(
        again.text.contains("asked only the groups that gave run"),
        "{}",
        again.text
    );
    assert!(again.refusal.is_none(), "{:?}", again.refusal);
    // Every group has answered now, so the change gets no further run.
    let refused = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(refused.contains(landing::ONE_REVIEW), "{refused}");
}

#[test]
fn a_reviewer_that_skips_a_rule_is_a_failed_review() {
    let demo = demo("review-skipped");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer("alpha", &[]);
    let reported = run_review(&demo, &fake, &side, "k");
    assert!(
        reported.text.contains("no verdict for rule-a"),
        "{}",
        reported.text
    );
    assert!(reported.refusal.is_some());
}

#[test]
fn settling_records_who_and_why_and_refuses_a_second_settling() {
    let demo = demo("review-settle");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let first = run_review(&demo, &fake, &side, "k");
    let finding = findings_of(&demo, first.landing.unwrap())[0].id;
    settle_as(&demo, finding, "confirmed", "real");
    let again = demo.store.write(&stamp(), "again", |db| {
        record::settle(db, finding, "dropped", "changed my mind", NOON, "tester")
    });
    assert!(
        again
            .unwrap_err()
            .text()
            .contains("settled already, as confirmed")
    );
    let bad = demo.store.write(&stamp(), "bad", |db| {
        record::settle(db, finding, "maybe", "x", NOON, "t")
    });
    assert!(bad.is_err());
    let empty = demo.store.write(&stamp(), "empty", |db| {
        record::settle(db, finding, "dropped", "  ", NOON, "t")
    });
    assert!(empty.is_err());
}

#[test]
fn a_change_with_nothing_past_the_base_reviews_nothing() {
    let demo = demo("review-empty");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let reported = run_review(&demo, &fake, &side, "k");
    assert!(reported.text.contains("nothing to review"));
    assert!(fake.prompts().is_empty());
}

#[test]
fn a_confirmed_finding_becomes_a_case_and_an_unconfirmed_one_does_not() {
    let demo = demo("review-case");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let reported = run_review(&demo, &fake, &side, "k");
    let landing = reported.landing.unwrap();
    let finding = findings_of(&demo, landing)[0].id;
    let out = demo.scratch.dir.join("cases-root");
    let words: Vec<String> = [
        "add",
        &landing.to_string(),
        &finding.to_string(),
        "--repo-dir",
        side.to_str().unwrap(),
        "--rules-root",
        out.to_str().unwrap(),
        "--project",
        "demo",
        "--by",
        "tester",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let refused = case_command(&demo.tool, &words).unwrap_err().text();
    assert!(refused.contains("not settled as confirmed"), "{refused}");
    settle_as(&demo, finding, "confirmed", "a real break");
    case_command(&demo.tool, &words).unwrap();
    let cases = cases::load(&out).unwrap();
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].project, "demo");
    assert_eq!(cases[0].expected[0].rule, "rule-a");
    assert_eq!(cases[0].expected[0].file, "a.rs");
    assert!(cases[0].patch.contains("+a.rs"));
}

#[test]
fn a_review_of_a_given_head_reads_a_copy_and_keeps_what_each_reviewer_was_sent_and_answered() {
    let demo = demo("review-head");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let head = crate::checkout::git(&side)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8(head.stdout).unwrap().trim().to_string();
    let reported = review(&Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: &side,
        base: "main",
        head: Some(&head),
        key: "k",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        brief: None,
    })
    .unwrap();
    assert!(reported.refusal.is_none());
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let (system, message, output, tokens): (String, String, String, i64) = conn
        .query_row(
            "SELECT s.text, m.text, e.output, e.tokens_in FROM review_exchange e JOIN review_text s ON s.hash = e.system_hash JOIN review_text m ON m.hash = e.message_hash",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert!(
        system.contains("Never do the bad thing here.") && system.contains("Demo project facts.")
    );
    assert!(message.contains("+a.rs"));
    assert!(output.contains("rule-a"));
    assert_eq!(tokens, 100);
    let version: String = conn
        .query_row("SELECT rules_version FROM review_run", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, "", "the fake rules folder is no repo");
}
