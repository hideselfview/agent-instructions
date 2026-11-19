use super::*;
use crate::step_failure::{RunResult, StepFailure};

fn run(kind: StepKind, name: &str, passed: bool, state: &str) -> Attempt {
    Attempt {
        kind,
        name: name.into(),
        at: 0,
        result: if passed {
            RunResult::Passed
        } else {
            RunResult::Failed(Some(StepFailure::default()))
        },
        state: Some(state.into()),
    }
}

#[test]
fn a_failed_run_is_fixed_flaky_or_standing_by_the_next_pass_of_the_same_step() {
    let attempts = vec![
        run(StepKind::Hook, "clippy", false, "a"),
        run(StepKind::Hook, "clippy", true, "b"),
        run(StepKind::Hook, "fmt", false, "b"),
        run(StepKind::Cargo, "fmt", true, "b"),
        run(StepKind::Hook, "fmt", true, "b"),
        run(StepKind::AppBuild, "mac build", false, "b"),
    ];
    let found: Vec<(&str, Outcome)> = failure_outcomes(&attempts)
        .into_iter()
        .map(|(run, outcome)| (run.name.as_str(), outcome))
        .collect();
    assert_eq!(
        found,
        vec![
            ("clippy", Outcome::Fixed),
            ("fmt", Outcome::Flaky),
            ("mac build", Outcome::Stood),
        ]
    );
}

#[test]
fn the_text_counts_each_kind_the_names_that_failed_and_each_groups_findings() {
    let mut failing = run(StepKind::Cargo, "cargo nextest run", false, "a");
    failing.result = RunResult::Failed(Some(StepFailure::new(None, ["a::one".to_string()])));
    let mut check = run(StepKind::Hook, "rustfmt", false, "a");
    check.result = RunResult::Failed(Some(StepFailure::new(None, ["src/a.rs".to_string()])));
    let mut unkept = run(StepKind::AppBuild, "mac build", false, "a");
    unkept.result = RunResult::Failed(None);
    let attempts = vec![
        failing,
        run(StepKind::Cargo, "cargo nextest run", true, "b"),
        check,
        unkept,
    ];
    let findings = vec![
        Finding {
            group: "Errors".into(),
            settled: Some("confirmed".into()),
            closed: true,
        },
        Finding {
            group: "Errors".into(),
            settled: Some("dropped".into()),
            closed: false,
        },
    ];
    let text = text(&[(&attempts, &findings)]);
    assert!(
        text.contains("cargo runs: 2 runs, 1 failed, 1 fixed, 0 flaky, 0 stood"),
        "{text}"
    );
    assert!(text.contains("hook checks: 1 runs, 1 failed"), "{text}");
    assert!(
        text.contains(
            "app build steps: 1 runs, 1 failed, 0 fixed, 0 flaky, 1 stood, 1 with no output kept"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "tests that cargo runs reported failed most often (failed runs)\n      1  a::one"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "staged files that hook checks failed on most often (failed runs)\n      1  src/a.rs"
        ),
        "{text}"
    );
    assert!(
        text.contains("   1 confirmed     1 dropped     0 not settled     1 closed  Errors"),
        "{text}"
    );
}
