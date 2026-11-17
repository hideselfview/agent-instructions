use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{Fake, Raised, fake, ten_lines};

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
    let conn = demo.store.open_existing().unwrap();
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
    // A fix commit is part of the net change: a new run reads it, and only
    // it, once.
    let fix = demo.commit(&side, "fix.rs");
    let commit = landing::fix_commit(&side, &fix).unwrap();
    let stamp = stamp();
    demo.store
        .write(&stamp, "fixed", |db| {
            record::record_fix(db, finding, &commit, stamp.at, "tester")
        })
        .unwrap();
    fake.answer("alpha", &[("rule-a", None)]);
    let read = run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{}", read.text);
    assert!(
        prompts[0].1.contains("+fix.rs") && !prompts[0].1.contains("+a.rs"),
        "{}",
        prompts[0].1
    );
    let again = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(fake.prompts().is_empty());
    assert!(again.contains(landing::ONE_REVIEW), "{again}");
    let conn = demo.store.open_existing().unwrap();
    assert_eq!(
        landing::review_runs(&conn, first.landing.unwrap())
            .unwrap()
            .len(),
        2
    );
}

/// Writes <text> to f.rs in <top>, commits it with <message>, and returns
/// the commit.
fn commit_f(top: &Path, text: &str, message: &str) -> String {
    std::fs::write(top.join("f.rs"), text).unwrap();
    crate::testing::git(top, &["add", "f.rs"]);
    crate::testing::git(top, &["commit", "-q", "-m", message]);
    crate::checkout::git_text(
        crate::checkout::git(top).args(["rev-parse", "HEAD"]),
        "read HEAD",
    )
    .unwrap()
    .trim()
    .to_string()
}

/// A demo whose main holds f.rs on origin too, a side worktree on branch work
/// whose one commit changes lines 2 and 8 of f.rs, and that commit's review.
fn reviewed_f(name: &str) -> (Demo, Fake, PathBuf, Reported) {
    let demo = demo(name);
    let fake = fake(&demo.scratch.dir);
    commit_f(&demo.main, &ten_lines(&[]), "Add f");
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    let side = demo.add_side(1, "agent", "work");
    commit_f(
        &side,
        &ten_lines(&[(2, "two-branch"), (8, "eight-branch")]),
        "Change f",
    );
    let first = run_review(&demo, &fake, &side, "k");
    assert_eq!(fake.prompts().len(), 1);
    (demo, fake, side, first)
}

#[test]
fn a_rebase_that_changes_no_line_of_the_branch_reads_nothing() {
    let (demo, fake, side, _) = reviewed_f("review-rebase-same");
    commit_f(
        &demo.main,
        &ten_lines(&[(5, "five-main")]),
        "Change f on main",
    );
    demo.commit(&demo.main, "m.rs");
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    crate::testing::git(&side, &["rebase", "-q", "origin/main"]);
    let refused = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(fake.prompts().is_empty());
    assert!(
        refused.contains(landing::ONE_REVIEW)
            && refused.contains("so builds review does not review it again"),
        "{refused}"
    );
}

#[test]
fn a_rebase_whose_conflict_resolution_changes_lines_reads_only_those_lines() {
    let (demo, fake, side, first) = reviewed_f("review-rebase-resolved");
    commit_f(
        &demo.main,
        &ten_lines(&[(8, "eight-main")]),
        "Change f on main",
    );
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    // The rebase stops on line 8, and its resolution keeps line 2.
    crate::testing::git(&side, &["reset", "-q", "--hard", "origin/main"]);
    commit_f(
        &side,
        &ten_lines(&[(2, "two-branch"), (8, "resolved")]),
        "Change f",
    );
    let second = run_review(&demo, &fake, &side, "k");
    assert_eq!(second.landing, first.landing);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    let read = &prompts[0].1;
    assert!(
        read.contains("-eight-branch")
            && read.contains("+resolved")
            && !read.contains("two-branch"),
        "{read}"
    );
    assert!(
        second
            .text
            .contains("read only how the branch's net change differs"),
        "{}",
        second.text
    );
    assert!(second.refusal.is_none(), "{:?}", second.refusal);
    let refused = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(fake.prompts().is_empty());
    assert!(refused.contains(landing::ONE_REVIEW), "{refused}");
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
        refusal.contains("group alpha gave no answer over a diff the review read"),
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
fn settle_again_takes_no_verdict_and_always_drops() {
    let demo = demo("review-settle-again");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let landing = run_review(&demo, &fake, &side, "k").landing.unwrap();
    let finding = findings_of(&demo, landing)[0].id;
    settle_as(&demo, finding, "confirmed", "real");
    let words = |extra: &[&str]| -> Vec<String> {
        let mut words = vec![finding.to_string()];
        words.extend(extra.iter().map(|w| w.to_string()));
        words.extend(
            ["--why", "wrong", "--project", "demo", "--by", "tester"]
                .iter()
                .map(|w| w.to_string()),
        );
        words
    };
    assert!(
        settle(&demo.tool, &words(&["dropped", "--again"]))
            .unwrap_err()
            .text()
            .contains("usage"),
        "--again takes no verdict"
    );
    settle(&demo.tool, &words(&["--again"])).unwrap();
    assert!(findings_of(&demo, landing)[0].settled_as("dropped"));
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
fn every_review_command_fails_for_a_project_with_no_store() {
    let demo = demo("review-no-store");
    let main = demo.main.display().to_string();
    let rules = demo.scratch.dir.display().to_string();
    let commands: Vec<Vec<&str>> = vec![
        vec![&main, "--by", "tester"],
        vec!["check-landing", &main, "HEAD"],
        vec!["landed", &main, "HEAD"],
        vec!["settle", "1", "confirmed", "--why", "real"],
        vec!["fixed", "1", "HEAD"],
        vec!["show", "1"],
        vec!["misses"],
        vec!["rates"],
        vec!["scores"],
        vec!["audit", "1", "--repo-dir", &main],
        vec!["replay", "--landing", "1", "--repo-dir", &main],
        vec![
            "case",
            "add",
            "--rule",
            "rule-a",
            "--file",
            "a.rs",
            "--commits",
            "main..main",
            "--name",
            "n",
            "--repo-dir",
            &main,
            "--rules-root",
            &rules,
        ],
    ];
    for command in commands {
        let mut args: Vec<String> = command.iter().map(|word| word.to_string()).collect();
        args.extend(["--project".to_string(), "ghost".to_string()]);
        let failed = cmd_review(&demo.tool, &args).unwrap_err().text();
        assert_eq!(
            failed,
            "there is no store for project ghost",
            "builds review {}",
            command.join(" ")
        );
    }
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
    let conn = demo.store.open_existing().unwrap();
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
