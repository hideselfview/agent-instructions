use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};

/// A reviewer stand-in and the checkout of rules it reviews against, in the
/// scratch folder of <demo>.
struct Fake {
    root: PathBuf,
    dir: PathBuf,
    program: PathBuf,
}

const GROUPS: &str = r#"
[[group]]
name = "alpha"
question = "q"
model = "sonnet"
rules = ["rule-a"]

[[group]]
name = "beta"
question = "q"
model = "opus"
rules = ["rule-b"]
"#;

fn fake(demo: &Demo) -> Fake {
    let root = demo.scratch.dir.join("instructions");
    let dir = demo.scratch.dir.join("reviewer");
    for sub in ["rules", "review", "projects", "principles"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    std::fs::create_dir_all(dir.join("calls")).unwrap();
    let rule = |slug: &str, glob: &str, blocking: bool, body: &str| {
        std::fs::write(
            root.join("rules").join(format!("{slug}.md")),
            format!("---\ndigest: d\npaths:\n  - '{glob}'\nblocking: {blocking}\n---\n\n{body}\n"),
        )
        .unwrap();
    };
    rule(
        "rule-a",
        "**/*.rs",
        true,
        "**Rule A.** Never do the bad thing here.",
    );
    rule(
        "rule-b",
        "**/*.swift",
        false,
        "**Rule B.** Never do the other thing.",
    );
    std::fs::write(root.join("review/groups.toml"), GROUPS).unwrap();
    std::fs::write(root.join("projects/demo.md"), "Demo project facts.\n").unwrap();
    let program = dir.join("claude");
    let script = format!(
        "#!/bin/bash\nD='{d}'\ncat > \"$D/calls/$$.prompt\"\ngroup=$(sed -n 's/^Group: //p' \"$D/calls/$$.prompt\" | head -1)\nif [ -f \"$D/fail-$group\" ]; then echo boom >&2; exit 3; fi\ncat \"$D/answer-$group.json\"\n",
        d = dir.display()
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let fake = Fake { root, dir, program };
    fake.answer("alpha", &[("rule-a", None)]);
    fake.answer("beta", &[("rule-b", None)]);
    fake
}

/// A finding as a reviewer words it: clause, file, line, code, problem.
type Raised = (&'static str, &'static str, i64, &'static str, &'static str);

impl Fake {
    /// What the reviewer of <group> answers: a verdict per rule.
    fn answer(&self, group: &str, rules: &[(&str, Option<Raised>)]) {
        let entries: Vec<serde_json::Value> = rules
            .iter()
            .map(|(rule, finding)| match finding {
                None => serde_json::json!({"rule": rule, "verdict": "none", "findings": []}),
                Some((clause, file, line, code, problem)) => serde_json::json!({
                    "rule": rule, "verdict": "finding",
                    "findings": [{"clause": clause, "file": file, "line": line, "code": code, "problem": problem}]
                }),
            })
            .collect();
        let body = serde_json::json!({
            "is_error": false,
            "usage": {"input_tokens": 100, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 10},
            "structured_output": {"rules": entries}
        });
        std::fs::write(
            self.dir.join(format!("answer-{group}.json")),
            body.to_string(),
        )
        .unwrap();
    }

    fn fail(&self, group: &str) {
        std::fs::write(self.dir.join(format!("fail-{group}")), "").unwrap();
    }

    /// The prompts the reviewer was run with since the last call of this, by group.
    fn prompts(&self) -> Vec<(String, String)> {
        let mut prompts = Vec::new();
        for entry in std::fs::read_dir(self.dir.join("calls")).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let group = text
                .lines()
                .find_map(|l| l.strip_prefix("Group: "))
                .unwrap()
                .to_string();
            prompts.push((group, text));
            std::fs::remove_file(path).unwrap();
        }
        prompts.sort();
        prompts
    }
}

fn stamp() -> Stamp {
    Stamp {
        at: NOON,
        by: "tester".to_string(),
    }
}

fn run_review(demo: &Demo, fake: &Fake, side: &Path, key: &str) -> Reported {
    review(&Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: side,
        base: "main",
        key,
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        brief: None,
    })
    .unwrap()
}

const BAD: Raised = (
    "Never do the bad thing here.",
    "a.rs",
    1,
    "a.rs",
    "The file does the bad thing.",
);

fn landing_closed(demo: &Demo, landing: i64) -> bool {
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    record::landing(&conn, landing).unwrap().2
}

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
    let fake = fake(&demo);
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
    assert!(landing_closed(&demo, reported.landing.unwrap()));
}

#[test]
fn a_finding_whose_quote_is_not_in_the_diff_is_dropped_and_recorded_as_dropped() {
    let demo = demo("review-quotes");
    let fake = fake(&demo);
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
fn a_blocking_finding_refuses_and_a_rerun_runs_the_groups_that_found_something_with_what_was_settled()
 {
    let demo = demo("review-rerun");
    let fake = fake(&demo);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    demo.commit(&side, "b.swift");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let first = run_review(&demo, &fake, &side, "k");
    assert_eq!(fake.prompts().len(), 2);
    let refusal = first.refusal.expect("a blocking finding refuses");
    assert!(
        refusal.contains("builds review settle <finding> confirmed|dropped --why TEXT"),
        "{refusal}"
    );
    assert!(refusal.contains("rule-a"));
    assert!(!landing_closed(&demo, first.landing.unwrap()));
    let finding = findings_of(&demo, first.landing.unwrap())[0].id;
    settle_as(
        &demo,
        finding,
        "dropped",
        "the fixture does this on purpose",
    );
    // The same finding again, worded differently, on the rerun.
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some((
                "Never do the bad thing here.",
                "a.rs",
                1,
                "a.rs",
                "Another wording of the problem.",
            )),
        )],
    );
    let second = run_review(&demo, &fake, &side, "k");
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "only the group that found something runs");
    assert_eq!(prompts[0].0, "alpha");
    assert!(
        prompts[0]
            .1
            .contains("settled dropped: the fixture does this on purpose"),
        "{}",
        prompts[0].1
    );
    assert!(second.text.contains("rerun"));
    assert!(second.refusal.is_none(), "{:?}", second.refusal);
    assert_eq!(second.landing, first.landing);
    assert!(landing_closed(&demo, second.landing.unwrap()));
    let again = findings_of(&demo, second.landing.unwrap());
    let inherited = again.last().unwrap();
    assert_eq!(inherited.settled.as_ref().unwrap().0, "dropped");
    assert!(
        inherited
            .settled
            .as_ref()
            .unwrap()
            .1
            .contains(&format!("same finding as {finding}"))
    );
}

#[test]
fn a_confirmed_finding_still_refuses_until_a_rerun_comes_back_clean() {
    let demo = demo("review-confirmed");
    let fake = fake(&demo);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let first = run_review(&demo, &fake, &side, "k");
    let finding = findings_of(&demo, first.landing.unwrap())[0].id;
    settle_as(&demo, finding, "confirmed", "it is a real break");
    let still = run_review(&demo, &fake, &side, "k");
    assert!(still.refusal.is_some());
    fake.answer("alpha", &[("rule-a", None)]);
    let clean = run_review(&demo, &fake, &side, "k");
    assert!(clean.refusal.is_none());
    assert!(landing_closed(&demo, clean.landing.unwrap()));
}

#[test]
fn a_reviewer_that_fails_refuses_the_landing_and_says_why() {
    let demo = demo("review-failed");
    let fake = fake(&demo);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.fail("alpha");
    let reported = run_review(&demo, &fake, &side, "k");
    let refusal = reported.refusal.expect("a failed review refuses");
    assert!(
        refusal.contains("review of group alpha failed"),
        "{refusal}"
    );
    assert!(refusal.contains("boom"), "{refusal}");
    assert!(!landing_closed(&demo, reported.landing.unwrap()));
}

#[test]
fn a_reviewer_that_skips_a_rule_is_a_failed_review() {
    let demo = demo("review-skipped");
    let fake = fake(&demo);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer("alpha", &[]);
    let reported = run_review(&demo, &fake, &side, "k");
    assert!(reported.refusal.unwrap().contains("no verdict for rule-a"));
}

#[test]
fn settling_records_who_and_why_and_refuses_a_second_settling() {
    let demo = demo("review-settle");
    let fake = fake(&demo);
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
    let fake = fake(&demo);
    let side = demo.add_side(1, "agent", "work");
    let reported = run_review(&demo, &fake, &side, "k");
    assert!(reported.text.contains("nothing to review"));
    assert!(fake.prompts().is_empty());
}
