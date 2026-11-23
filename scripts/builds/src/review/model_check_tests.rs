use super::*;
use crate::review::fakes::fake;
use crate::testing::Scratch;

fn finding(rule: &str, file: &str) -> Finding {
    Finding {
        id: 1,
        run_id: 1,
        kind: "audit".to_string(),
        landing_id: Some(1),
        group: "g".to_string(),
        rule: rule.to_string(),
        clause: "Never do the bad thing here.".to_string(),
        file: file.to_string(),
        line: crate::review::record::FindingLine::Verified(crate::review::check::Position {
            side: crate::review::check::Side::After,
            line: 1,
        }),
        code: "a.rs".to_string(),
        problem: "The file does the bad thing.".to_string(),
        blocking: true,
        check_failed: None,
        settled: None,
        closed_by: None,
    }
}

fn diff() -> Diff {
    let text = "diff --git a/a.rs b/a.rs\n--- /dev/null\n+++ b/a.rs\n@@ -0,0 +1 @@\n+a.rs\n";
    Diff {
        messages: "Add a.rs\n".to_string(),
        source: None,
        text: text.to_string(),
        changed: vec!["a.rs".to_string()],
    }
}

#[test]
fn the_model_reads_the_rule_and_the_finding_and_its_answer_settles_the_finding() {
    let scratch = Scratch::new("model-check");
    let fake = fake(&scratch.dir);
    let material = Material::load(&fake.root, "demo").unwrap();
    fake.answer_task(
        "model-check rule-a",
        json!({"verdict": "confirmed", "why": "a.rs does the bad thing."}),
    );
    let checked = check(
        &fake.program,
        &Selection::Named("opus".to_string()),
        &material,
        &finding("rule-a", "a.rs"),
        &diff(),
    )
    .unwrap();
    assert_eq!(
        checked.settle_why(),
        Ok((
            "confirmed",
            "the opus check confirmed it: a.rs does the bad thing.".to_string()
        ))
    );
    assert_eq!(checked.usage.tokens.input, Some(50));
    let prompt = fake.dir.join("calls");
    let sent: String = std::fs::read_dir(prompt)
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert!(sent.contains("**Rule A.** Never do the bad thing here."));
    assert!(sent.contains("+a.rs"));
    fake.answer_task(
        "model-check rule-a",
        json!({"verdict": "rejected", "why": "The clause does not cover a.rs."}),
    );
    let rejected = check(
        &fake.program,
        &Selection::Named("opus".to_string()),
        &material,
        &finding("rule-a", "a.rs"),
        &diff(),
    )
    .unwrap();
    assert_eq!(rejected.settle_why().unwrap().0, "dropped");
}

#[test]
fn a_finding_on_the_commit_messages_is_checked_against_them() {
    let scratch = Scratch::new("model-check-message");
    let fake = fake(&scratch.dir);
    let material = Material::load(&fake.root, "demo").unwrap();
    fake.answer_task(
        "model-check rule-a",
        json!({"verdict": "confirmed", "why": "w"}),
    );
    let checked = check(
        &fake.program,
        &Selection::Named("opus".to_string()),
        &material,
        &finding("rule-a", files::COMMIT_MESSAGE),
        &diff(),
    )
    .unwrap();
    assert!(checked.verdict.is_ok(), "{checked:?}");
}

#[test]
fn a_check_with_no_rule_text_or_no_file_or_no_answer_settles_nothing() {
    let scratch = Scratch::new("model-check-failed");
    let fake = fake(&scratch.dir);
    let material = Material::load(&fake.root, "demo").unwrap();
    let no_rule = check(
        &fake.program,
        &Selection::Named("opus".to_string()),
        &material,
        &finding("rule-z", "a.rs"),
        &diff(),
    );
    assert_eq!(
        no_rule.unwrap_err(),
        "the rules checkout has no rule rule-z"
    );
    let no_file = check(
        &fake.program,
        &Selection::Named("opus".to_string()),
        &material,
        &finding("rule-a", "z.rs"),
        &diff(),
    );
    assert_eq!(no_file.unwrap_err(), "the diff has no file z.rs");
    fake.fail("model-check rule-a");
    let failed = check(
        &fake.program,
        &Selection::Named("opus".to_string()),
        &material,
        &finding("rule-a", "a.rs"),
        &diff(),
    )
    .unwrap();
    assert!(failed.verdict.is_err());
}
