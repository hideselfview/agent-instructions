use super::*;
use crate::review::cases::Addition;
use crate::review::fakes::{Fake, Raised, fake};
use crate::testing::Scratch;

/// A change to a.rs and b.swift: both groups run on it.
const PATCH: &str = "From 1234 Mon Sep 17 00:00:00 2001\nSubject: [PATCH] x\n\n---\n\ndiff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1 @@\n+a.rs\ndiff --git a/b.swift b/b.swift\nindex 1..2 100644\n--- a/b.swift\n+++ b/b.swift\n@@ -0,0 +1 @@\n+b.swift\n-- \n2.50\n";

const BAD: Raised = (
    "Never do the bad thing here.",
    "a.rs",
    1,
    "a.rs",
    "The file does the bad thing.",
);

fn setup(name: &str) -> (Scratch, Fake) {
    let scratch = Scratch::new(name);
    let fake = fake(&scratch.dir);
    cases::add(
        &fake.root,
        "both",
        "demo",
        PATCH,
        Addition::Finding {
            rule: "rule-a",
            file: "a.rs",
        },
    )
    .unwrap();
    // The current grouping finds the expected finding.
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    (scratch, fake)
}

fn merged(name: &str) -> String {
    format!(
        "[[group]]\nname = \"{name}\"\nquestion = \"q\"\nmodel = \"sonnet\"\nrules = [\"rule-a\", \"rule-b\"]\n"
    )
}

fn propose(fake: &Fake, proposals: &[(&str, &str)]) {
    let list: Vec<serde_json::Value> = proposals
        .iter()
        .map(|(toml, why)| serde_json::json!({"groups_toml": toml, "why": why}))
        .collect();
    fake.answer_task("propose groupings", serde_json::json!({"proposals": list}));
}

#[test]
fn the_cheapest_grouping_that_loses_no_recall_is_adopted_with_its_numbers() {
    let (_scratch, fake) = setup("optimize-adopt");
    // One group for both rules: one call where there were two.
    fake.answer("both-rules", &[("rule-a", Some(BAD)), ("rule-b", None)]);
    // Three groups cost more and one that loses the finding does not qualify.
    fake.answer("lossy", &[("rule-a", None), ("rule-b", None)]);
    propose(
        &fake,
        &[
            (&merged("lossy"), "lossy loses the finding"),
            (&merged("both-rules"), "one call covers both rules"),
        ],
    );
    let decision = optimize(&fake.root, &fake.program, &fake.program, &[]).unwrap();
    assert!(
        decision
            .text
            .contains("proposal 1: 1 groups, 0 of 1 expected findings found"),
        "{}",
        decision.text
    );
    assert!(decision.text.contains("does not qualify"));
    assert!(
        decision.text.contains("adopting proposal 2"),
        "{}",
        decision.text
    );
    let adoption = decision.adopt.expect("a cheaper grouping qualifies");
    assert_eq!(adoption.groups_toml, merged("both-rules"));
    assert!(adoption.message.contains("Before: 2 groups, 1 of 1 expected findings found, 0 false blocking findings, 200 tokens in, 20 out."), "{}", adoption.message);
    assert!(adoption.message.contains("After: 1 groups, 1 of 1 expected findings found, 0 false blocking findings, 100 tokens in, 10 out."), "{}", adoption.message);
    assert!(adoption.message.contains("Why: one call covers both rules"));
}

#[test]
fn nothing_changes_when_no_proposal_is_valid_cheaper_and_whole() {
    let (_scratch, fake) = setup("optimize-none");
    // Missing rule-b: not whole. Same two groups: not cheaper.
    let missing =
        "[[group]]\nname = \"x\"\nquestion = \"q\"\nmodel = \"sonnet\"\nrules = [\"rule-a\"]\n";
    propose(
        &fake,
        &[
            (missing, "drops a rule"),
            (crate::review::fakes::GROUPS, "the same"),
        ],
    );
    let decision = optimize(&fake.root, &fake.program, &fake.program, &[]).unwrap();
    assert!(
        decision.text.contains("proposal 1 discarded"),
        "{}",
        decision.text
    );
    assert!(decision.text.contains("rule-b is in no group"));
    assert!(decision.text.contains("nothing changes"));
    assert!(decision.adopt.is_none());
}

#[test]
fn a_current_grouping_that_misses_a_finding_is_not_compared_against() {
    let (_scratch, fake) = setup("optimize-baseline");
    fake.answer("alpha", &[("rule-a", None)]);
    propose(&fake, &[]);
    let decision = optimize(&fake.root, &fake.program, &fake.program, &[]).unwrap();
    assert!(
        decision.text.contains("misses an expected finding"),
        "{}",
        decision.text
    );
    assert!(decision.adopt.is_none());
}

#[test]
fn a_candidate_with_more_false_blocking_findings_does_not_qualify() {
    let t = |found, false_blocking, tokens_in| Totals {
        expected: 2,
        found,
        false_blocking,
        failed_calls: 0,
        tokens_in,
        tokens_out: 0,
    };
    let baseline = t(2, 0, 100);
    assert!(qualifies(&baseline, &t(2, 0, 90)));
    assert!(!qualifies(&baseline, &t(2, 1, 50)));
    assert!(!qualifies(&baseline, &t(1, 0, 50)));
    assert!(!qualifies(&baseline, &t(2, 0, 100)));
    assert_eq!(
        best(&baseline, &[t(2, 0, 90), t(2, 0, 60), t(1, 0, 10)]),
        Some(1)
    );
    assert_eq!(best(&baseline, &[t(2, 1, 10)]), None);
}

#[test]
fn an_adopted_grouping_is_committed_and_a_dirty_groups_file_is_refused() {
    let scratch = Scratch::new("optimize-commit");
    let repo = scratch.dir.join("repo");
    std::fs::create_dir_all(repo.join("review")).unwrap();
    let git = |args: &[&str]| crate::testing::git(&repo, args);
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("review/groups.toml"), "old\n").unwrap();
    git(&["add", "review/groups.toml"]);
    git(&["commit", "-q", "-m", "first"]);
    let adoption = Adoption {
        groups_toml: "new\n".to_string(),
        message: "Regroup the review's rules\n\nBefore and after numbers.\n".to_string(),
    };
    adopt(&repo, &adoption).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join("review/groups.toml")).unwrap(),
        "new\n"
    );
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["log", "-1", "--format=%B"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).contains("Before and after numbers."));
    std::fs::write(repo.join("review/groups.toml"), "dirty\n").unwrap();
    assert!(
        adopt(&repo, &adoption)
            .unwrap_err()
            .text()
            .contains("not committed")
    );
}
