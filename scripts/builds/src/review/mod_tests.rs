use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{
    Fake, Raised, commit_text_group, fake, group_file, rule_file, style_group, style_site,
    ten_lines,
};

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
    // A fix commit is part of the changes since the last review: a new run
    // reads it, and only it, once, and closes the finding it fixes.
    demo.commit(&side, "fix.rs");
    closes(&fake, &[finding]);
    let read = run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{}", read.text);
    assert!(
        prompts[0].1.contains("+fix.rs")
            && !prompts[0].1.contains("+a.rs")
            && prompts[0]
                .1
                .contains(&format!("Finding {finding} (rule-a), a.rs:1")),
        "{}",
        prompts[0].1
    );
    assert!(
        read.text.contains(&format!("closed finding {finding}")),
        "{}",
        read.text
    );
    assert!(read.refusal.is_none(), "{:?}", read.refusal);
    assert!(
        findings_of(&demo, first.landing.unwrap())[0]
            .closed_by
            .is_some()
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

/// What the reviewer of group alpha answers: no finding for rule-a, and
/// <closed> closed.
fn closes(fake: &Fake, closed: &[i64]) {
    fake.answer_task(
        "alpha",
        serde_json::json!({
            "rules": [{"rule": "rule-a", "verdict": "none", "findings": []}],
            "closed": closed,
        }),
    );
}

/// The reviewer reads only the changes since the last review, and builds
/// drops a finding whose code is on a line outside them.
#[test]
fn a_finding_on_a_line_outside_the_changes_since_the_last_review_is_dropped() {
    let demo = demo("review-outside");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let first = run_review(&demo, &fake, &side, "k");
    demo.commit(&side, "c.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let later = run_review(&demo, &fake, &side, "k");
    let found = findings_of(&demo, first.landing.unwrap());
    assert_eq!(found.len(), 1, "{}", later.text);
    assert!(
        found[0]
            .check_failed
            .as_deref()
            .is_some_and(|why| why.contains("the diff has no file a.rs")),
        "{:?}",
        found[0]
    );
    assert!(later.refusal.is_none(), "{:?}", later.refusal);
}

/// Writes <text> to f.rs in <top>, commits it with <message>, and returns
/// the commit.
fn commit_f(top: &Path, text: &str, message: &str) -> String {
    std::fs::write(top.join("f.rs"), text).unwrap();
    crate::testing::git(top, &["add", "f.rs"]);
    crate::testing::git(top, &["commit", "-q", "-m", message]);
    boards::git_text(boards::git(top).args(["rev-parse", "HEAD"]), "read HEAD")
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

/// A later run reads the changes since the last review: the fix's line, with
/// the lines around it, one of which the first run read. builds drops the
/// finding on that line and keeps the one on the fix's line.
#[test]
fn a_later_runs_finding_on_a_line_the_fix_did_not_change_is_dropped() {
    let (demo, fake, side, first) = reviewed_f("review-unchanged-line");
    commit_f(
        &side,
        &ten_lines(&[(2, "two-branch"), (8, "eight-branch"), (9, "nine-fix")]),
        "Fix f",
    );
    let found = |code: &str| serde_json::json!({"clause": BAD.0, "file": "f.rs", "line": 1, "code": code, "problem": "bad"});
    fake.answer_task(
        "alpha",
        serde_json::json!({"rules": [{"rule": "rule-a", "verdict": "finding", "findings": [found("eight-branch"), found("nine-fix")]}]}),
    );
    run_review(&demo, &fake, &side, "k");
    let checked: Vec<(String, Option<String>)> = findings_of(&demo, first.landing.unwrap())
        .into_iter()
        .map(|f| (f.code, f.check_failed))
        .collect();
    assert_eq!(
        checked,
        [
            (
                "eight-branch".to_string(),
                Some(
                    "its code sits only on lines of f.rs around the diff's changes, which the diff does not add or remove"
                        .to_string()
                )
            ),
            ("nine-fix".to_string(), None),
        ]
    );
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
            .contains("read only the changes since the last review"),
        "{}",
        second.text
    );
    assert!(second.refusal.is_none(), "{:?}", second.refusal);
    let refused = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(fake.prompts().is_empty());
    assert!(refused.contains(landing::ONE_REVIEW), "{refused}");
}

/// Before builds read the changes since the last review, a later run of a
/// review read only a stretch of the branch's commits. The landing does not
/// count such a run as its last review.
#[test]
fn a_run_that_read_a_stretch_of_commits_is_not_the_last_review() {
    let demo = demo("review-stretch");
    let fake = fake(&demo.scratch.dir);
    commit_f(&demo.main, &ten_lines(&[]), "Add f");
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    let side = demo.add_side(1, "agent", "work");
    let change_f = || {
        commit_f(
            &side,
            &ten_lines(&[(2, "two-branch"), (8, "eight-branch")]),
            "Change f",
        )
    };
    let changed = change_f();
    let g = demo.commit(&side, "g.rs");
    let landing = run_review(&demo, &fake, &side, "k").landing.unwrap();
    fake.prompts();
    let stamp = stamp();
    demo.store
        .write(&stamp, "stretch", |db| {
            record::add_run(
                db,
                &record::NewRun {
                    landing: Some(landing),
                    kind: Kind::Review { changed_lines: 0 },
                    label: None,
                    at: stamp.at,
                    base: &changed,
                    head: &g,
                    since: None,
                    rules_version: "",
                },
                &[],
            )
            .map(|_| ())
        })
        .unwrap();
    commit_f(
        &demo.main,
        &ten_lines(&[(5, "five-main")]),
        "Change f on main",
    );
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    crate::testing::git(&side, &["rebase", "-q", "origin/main"]);
    let refused = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(fake.prompts().is_empty());
    assert!(refused.contains(landing::ONE_REVIEW), "{refused}");
    // A rebase whose resolution changes line 8 reads only that line.
    crate::testing::git(&side, &["reset", "-q", "--hard", "origin/main"]);
    commit_f(
        &side,
        &ten_lines(&[(2, "two-branch"), (5, "five-main"), (8, "resolved")]),
        "Change f",
    );
    demo.commit(&side, "g.rs");
    run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    let read = &prompts[0].1;
    assert!(
        read.contains("-eight-branch")
            && read.contains("+resolved")
            && !read.contains("two-branch")
            && !read.contains("+g.rs"),
        "{read}"
    );
    // A commit added after that reads only its own change.
    demo.commit(&side, "h.rs");
    run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    let read = &prompts[0].1;
    assert!(
        read.contains("+h.rs") && !read.contains("resolved") && !read.contains("+g.rs"),
        "{read}"
    );
}

/// What the reviewer of <group> answers: a finding of <rule>, quoting
/// <clause>, in each of <files>.
fn raise(fake: &Fake, group: &str, rule: &str, clause: &str, files: &[&str]) {
    let findings: Vec<serde_json::Value> = files
        .iter()
        .map(|file| {
            serde_json::json!({"clause": clause, "file": file, "line": 1, "code": file, "problem": "p"})
        })
        .collect();
    let verdict = if findings.is_empty() {
        "none"
    } else {
        "finding"
    };
    fake.answer_task(
        group,
        serde_json::json!({"rules": [{"rule": rule, "verdict": verdict, "findings": findings}]}),
    );
}

/// Makes a demo whose branch adds a.rs, b.rs and c.rs. Groups writing and
/// alpha both cover these files. A first review raises a writing finding in
/// each of <writing> and an alpha finding in each of <alpha>, and the test
/// settles every finding as confirmed. The last item of the result is the
/// review's landing.
fn found_in(name: &str, writing: &[&str], alpha: &[&str]) -> (Demo, Fake, PathBuf, i64) {
    let demo = demo(name);
    let fake = fake(&demo.scratch.dir);
    std::fs::create_dir_all(fake.root.join("rules/writing")).unwrap();
    rule_file(
        &fake.root,
        "writing",
        "rule-w",
        "**/*.rs",
        false,
        "**Rule W.** Write plain sentences.",
    );
    group_file(&fake.root, "writing", "writing", "opus", "");
    let side = demo.add_side(1, "agent", "work");
    for file in ["a.rs", "b.rs", "c.rs"] {
        demo.commit(&side, file);
    }
    raise(
        &fake,
        "writing",
        "rule-w",
        "Write plain sentences.",
        writing,
    );
    raise(
        &fake,
        "alpha",
        "rule-a",
        "Never do the bad thing here.",
        alpha,
    );
    let landing = run_review(&demo, &fake, &side, "k").landing.unwrap();
    for finding in findings_of(&demo, landing) {
        settle_as(&demo, finding.id, "confirmed", "real");
    }
    fake.prompts();
    fake.answer("writing", &[("rule-w", None)]);
    fake.answer("alpha", &[("rule-a", None)]);
    (demo, fake, side, landing)
}

/// Commits a change to each of <files>.
fn fix(side: &Path, files: &[&str]) {
    for file in files {
        std::fs::write(side.join(file), format!("{file} fixed\n")).unwrap();
        crate::testing::git(side, &["add", file]);
    }
    crate::testing::git(side, &["commit", "-q", "-m", "Fix the findings"]);
}

/// Each group whose reviewer ran since the last call, with the files of
/// a.rs, b.rs and c.rs whose diff it read.
fn read_by_group(fake: &Fake) -> Vec<(String, Vec<&'static str>)> {
    fake.prompts()
        .into_iter()
        .map(|(group, prompt)| {
            let read = ["a.rs", "b.rs", "c.rs"]
                .into_iter()
                .filter(|file| prompt.contains(&format!("diff --git a/{file} b/{file}")))
                .collect();
            (group, read)
        })
        .collect()
}

#[test]
fn a_wording_fix_in_a_file_with_only_writing_findings_runs_only_the_writing_group() {
    let (demo, fake, side, _) = found_in("review-finder-wording", &["a.rs"], &[]);
    fix(&side, &["a.rs"]);
    let read = run_review(&demo, &fake, &side, "k");
    assert_eq!(
        read_by_group(&fake),
        vec![("writing".to_string(), vec!["a.rs"])],
        "{}",
        read.text
    );
    assert!(
        read.text.contains("alpha did not read a.rs"),
        "{}",
        read.text
    );
}

#[test]
fn a_file_where_two_groups_hold_findings_is_read_by_both() {
    let (demo, fake, side, _) = found_in("review-finder-both", &["a.rs"], &["a.rs"]);
    fix(&side, &["a.rs"]);
    run_review(&demo, &fake, &side, "k");
    assert_eq!(
        read_by_group(&fake),
        vec![
            ("alpha".to_string(), vec!["a.rs"]),
            ("writing".to_string(), vec!["a.rs"])
        ]
    );
}

#[test]
fn a_changed_file_with_no_open_finding_is_read_by_every_group_that_covers_it() {
    let (demo, fake, side, _) = found_in("review-finder-other", &["a.rs"], &[]);
    fix(&side, &["a.rs", "c.rs"]);
    run_review(&demo, &fake, &side, "k");
    assert_eq!(
        read_by_group(&fake),
        vec![
            ("alpha".to_string(), vec!["c.rs"]),
            ("writing".to_string(), vec!["a.rs", "c.rs"])
        ]
    );
}

#[test]
fn each_group_reads_only_the_files_where_it_holds_findings_and_both_run_in_one_run() {
    let (demo, fake, side, landing) =
        found_in("review-finder-example", &["a.rs", "b.rs"], &["c.rs"]);
    fix(&side, &["a.rs", "b.rs", "c.rs"]);
    run_review(&demo, &fake, &side, "k");
    assert_eq!(
        read_by_group(&fake),
        vec![
            ("alpha".to_string(), vec!["c.rs"]),
            ("writing".to_string(), vec!["a.rs", "b.rs"])
        ]
    );
    let conn = demo.store.open_existing().unwrap();
    let runs = landing::review_runs(&conn, landing).unwrap();
    assert_eq!(runs.len(), 2);
    let mut groups: Vec<String> = record::calls_of_run(&conn, runs[1].id)
        .unwrap()
        .into_iter()
        .map(|call| call.group)
        .collect();
    groups.sort();
    assert_eq!(groups, ["alpha", "writing"]);
}

/// A group's reviewer of a whole diff reads every file of it, so the group can
/// hold an open finding in a file its rules do not cover. A fix of that file
/// alone asks the group, which reads the fix and closes the finding.
#[test]
fn a_fix_of_a_file_no_rule_covers_asks_the_group_that_holds_its_finding() {
    let demo = demo("review-uncovered-fix");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    demo.commit(&side, "b.js");
    raise(
        &fake,
        "alpha",
        "rule-a",
        "Never do the bad thing here.",
        &["b.js"],
    );
    let landing = run_review(&demo, &fake, &side, "k").landing.unwrap();
    let found = findings_of(&demo, landing);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].check_failed, None);
    settle_as(&demo, found[0].id, "confirmed", "real");
    fake.prompts();
    fix(&side, &["b.js"]);
    closes(&fake, &[found[0].id]);
    let read = run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{}", read.text);
    let (group, prompt) = &prompts[0];
    assert_eq!(group, "alpha");
    assert!(
        prompt.contains("diff --git a/b.js b/b.js") && !prompt.contains("+a.rs"),
        "{prompt}"
    );
    assert!(
        read.text
            .contains(&format!("closed finding {}", found[0].id)),
        "{}",
        read.text
    );
    assert!(read.refusal.is_none(), "{:?}", read.refusal);
}

/// A group that lists sites reads one file per call, each a file its rules
/// cover. When the rules change between runs so that they no longer cover a
/// file where the group holds an open finding, a fix of that file still gives
/// the group a call over it, which closes the finding.
#[test]
fn a_group_that_lists_sites_reads_a_file_where_it_holds_a_finding_its_rules_no_longer_cover() {
    let demo = demo("review-sites-uncovered-fix");
    let fake = fake(&demo.scratch.dir);
    std::fs::create_dir_all(fake.root.join("rules/writing")).unwrap();
    let rule_w = |glob: &str| {
        rule_file(
            &fake.root,
            "writing",
            "rule-w",
            glob,
            true,
            "**Rule W.** Write plain sentences.",
        )
    };
    rule_w("**/*");
    group_file(
        &fake.root,
        "writing",
        "writing",
        "opus",
        "enumerate = true\nsites = \"is a sentence\"\n",
    );
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    demo.commit(&side, "b.js");
    fake.answer_task(
        "writing",
        serde_json::json!({"summary": "s", "sites": [{
            "file": "b.js", "line": 1, "code": "b.js", "verdict": "finding",
            "rule": "rule-w", "clause": "Write plain sentences.", "problem": "p"
        }]}),
    );
    let landing = run_review(&demo, &fake, &side, "k").landing.unwrap();
    let found = findings_of(&demo, landing);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].check_failed, None);
    settle_as(&demo, found[0].id, "confirmed", "real");
    fake.prompts();
    rule_w("**/*.rs");
    fix(&side, &["b.js"]);
    fake.answer_task(
        "writing",
        serde_json::json!({"summary": "s", "sites": [], "closed": [found[0].id]}),
    );
    let read = run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{}", read.text);
    let (group, prompt) = &prompts[0];
    assert_eq!(group, "writing");
    assert!(prompt.contains("diff --git a/b.js b/b.js"), "{prompt}");
    assert!(
        read.text
            .contains(&format!("closed finding {}", found[0].id)),
        "{}",
        read.text
    );
}

/// Group style judges no code comment, and group alpha judges comments too:
/// builds drops the style group's finding on a comment line with its reason,
/// and keeps its finding on a string and alpha's finding on the comment.
#[test]
fn a_style_finding_on_a_comment_line_is_dropped_and_an_accuracy_finding_there_stays() {
    let demo = demo("review-comment-style");
    let fake = fake(&demo.scratch.dir);
    style_group(
        &fake,
        serde_json::json!([
            style_site("a.rs", "the cover is uploaded"),
            style_site("a.rs", "\"Cover uploaded\""),
        ]),
    );
    let side = demo.add_side(1, "agent", "work");
    std::fs::write(
        side.join("a.rs"),
        "/// Sends the cover:\n/// the cover is uploaded\nfn send() -> &'static str {\n    \"Cover uploaded\"\n}\n",
    )
    .unwrap();
    crate::testing::git(&side, &["add", "a.rs"]);
    crate::testing::git(&side, &["commit", "-q", "-m", "Add a"]);
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some((
                BAD.0,
                "a.rs",
                2,
                "the cover is uploaded",
                "The comment is wrong.",
            )),
        )],
    );
    let landing = run_review(&demo, &fake, &side, "k").landing.unwrap();
    let found: Vec<(String, String, Option<String>)> = findings_of(&demo, landing)
        .into_iter()
        .map(|f| (f.group, f.code, f.check_failed))
        .collect();
    assert_eq!(
        found,
        [
            (
                "alpha".to_string(),
                "the cover is uploaded".to_string(),
                None
            ),
            (
                "style".to_string(),
                "the cover is uploaded".to_string(),
                Some(
                    "its code is only comment lines, and the group of rule rule-s judges no code comment"
                        .to_string()
                )
            ),
            ("style".to_string(), "\"Cover uploaded\"".to_string(), None),
        ]
    );
}

/// Makes a demo whose branch commits b.rs, then a.rs with the message "Add a
/// with a vague line". Group writing reads the commit messages, and its first
/// review raises a finding on that message, which the test settles as
/// confirmed. Returns the demo, its reviewer, the side worktree and the
/// finding.
fn found_in_a_message(name: &str) -> (Demo, Fake, PathBuf, i64) {
    let demo = demo(name);
    let fake = fake(&demo.scratch.dir);
    commit_text_group(&fake);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "b.rs");
    std::fs::write(side.join("a.rs"), "a.rs").unwrap();
    crate::testing::git(&side, &["add", "a.rs"]);
    crate::testing::git(&side, &["commit", "-q", "-m", "Add a with a vague line"]);
    fake.answer_task(
        "writing",
        serde_json::json!({"summary": "s", "sites": [{
            "file": files::COMMIT_MESSAGE, "line": 1, "code": "a vague line",
            "verdict": "finding", "clause": "Write plain sentences.", "problem": "p"
        }]}),
    );
    let landing = run_review(&demo, &fake, &side, "k").landing.unwrap();
    let found = findings_of(&demo, landing);
    assert_eq!(found.len(), 1, "{found:?}");
    settle_as(&demo, found[0].id, "confirmed", "real");
    fake.prompts();
    (demo, fake, side, found[0].id)
}

#[test]
fn a_review_of_a_reworded_message_reads_only_the_changed_messages_and_closes_the_finding() {
    let (demo, fake, side, finding) = found_in_a_message("review-reworded");
    crate::testing::git(&side, &["commit", "-q", "--amend", "-m", "Add a"]);
    fake.answer_task(
        "writing",
        serde_json::json!({"summary": "s", "sites": [], "closed": [finding]}),
    );
    let read = run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{}", read.text);
    let (group, prompt) = &prompts[0];
    assert_eq!(group, "writing");
    assert!(
        prompt.contains("diff --git a/COMMIT_MESSAGE")
            && prompt.contains("+Add a\n")
            && !prompt.contains("+Add a with a vague line")
            && !prompt.contains("+b.rs")
            && !prompt.contains("diff --git a/a.rs"),
        "{prompt}"
    );
    assert!(
        read.text.contains(&format!("closed finding {finding}")),
        "{}",
        read.text
    );
    assert!(read.refusal.is_none(), "{:?}", read.refusal);
    let again = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(fake.prompts().is_empty());
    assert!(again.contains(landing::ONE_REVIEW), "{again}");
}

#[test]
fn a_commit_amended_with_its_own_message_and_files_is_no_change_to_review() {
    let (demo, fake, side, _) = found_in_a_message("review-amended");
    crate::testing::git(
        &side,
        &[
            "commit",
            "-q",
            "--amend",
            "--no-edit",
            "--date=2000-01-01T00:00:00",
        ],
    );
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
fn only_by_person_allows_a_confirmed_finding_to_land() {
    let demo = demo("review-settle-allowed");
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
            [
                "--why",
                "the person: land it",
                "--project",
                "demo",
                "--by",
                "tester",
            ]
            .iter()
            .map(|w| w.to_string()),
        );
        words
    };
    assert!(
        settle(&demo.tool, &words(&["allowed"]))
            .unwrap_err()
            .text()
            .contains("never on your own judgment")
    );
    assert!(
        settle(&demo.tool, &words(&["dropped", "--by-person"]))
            .unwrap_err()
            .text()
            .contains("--by-person goes with allowed")
    );
    assert!(findings_of(&demo, landing)[0].is_open());
    settle(&demo.tool, &words(&["allowed", "--by-person"])).unwrap();
    let found = &findings_of(&demo, landing)[0];
    assert!(!found.is_open() && found.settled_as("confirmed"));
    let Some(record::Settled::Allowed { allowed, .. }) = &found.settled else {
        panic!("{:?}", found.settled)
    };
    assert_eq!(allowed.why, "the person: land it");
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
    let head = boards::git(&side)
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

#[test]
fn no_group_reviews_the_test_cases() {
    let demo = demo("review-test-cases");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let case = side.join(cases::DIR).join("one");
    std::fs::create_dir_all(&case).unwrap();
    std::fs::write(case.join("old.rs"), "the old code\n").unwrap();
    crate::testing::git(&side, &["add", "review"]);
    crate::testing::git(&side, &["commit", "-q", "-m", "Add a case"]);
    let only_cases = run_review(&demo, &fake, &side, "k");
    assert!(
        only_cases.text.contains("nothing to review"),
        "{}",
        only_cases.text
    );
    assert!(fake.prompts().is_empty());
    demo.commit(&side, "a.rs");
    run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].1.contains("+a.rs"));
    assert!(!prompts[0].1.contains(cases::DIR), "{}", prompts[0].1);
    // A later commit that changes only a case gives the review nothing to read.
    std::fs::write(case.join("old.rs"), "older code\n").unwrap();
    crate::testing::git(&side, &["commit", "-q", "-am", "Change the case"]);
    let refused = try_review(&demo, &fake, &side, "k").unwrap_err().text();
    assert!(
        refused.contains("nothing changed since the last review"),
        "{refused}"
    );
    assert!(fake.prompts().is_empty());
}
