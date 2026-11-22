use super::*;
use serde_json::json;

#[test]
fn codex_schema_requires_optional_fields_on_the_wire_and_decodes_null_as_omitted() {
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "list": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"rule": {"type": "string"}},
                },
            },
        },
        "required": ["list"],
    });
    require_properties(&mut schema);
    let item = &schema["properties"]["list"]["items"];
    assert_eq!(item["required"], json!(["rule"]));
    assert_eq!(
        item["properties"]["rule"],
        json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
    );
    let mut answer = json!({"list": [{"rule": null}, {"rule": "keep"}]});
    remove_null_properties(&mut answer);
    assert_eq!(answer, json!({"list": [{}, {"rule": "keep"}]}));
}

const COMPLETED: &str = r#"{"type":"turn.completed","usage":{"input_tokens":31,"cached_input_tokens":7,"cache_write_input_tokens":2,"output_tokens":12,"reasoning_output_tokens":5}}"#;

#[test]
fn codex_returns_the_final_structured_message_and_the_turns_usage() {
    let events = format!(
        "{}\n{}\n{COMPLETED}",
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"Reading context."}}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"claims\":[]}"}}"#,
    );
    let called = codex_events(events.as_bytes(), false);
    assert_eq!(called.body.unwrap(), json!({"claims": []}));
    assert_eq!(
        called.usage.unwrap(),
        Tokens {
            input: 31,
            output: 12,
            cached: 7,
            written: 2
        }
    );
}

#[test]
fn codex_rejects_a_command_in_a_check_that_has_no_tools() {
    let events = format!(
        "{}\n{}\n{COMPLETED}",
        r#"{"type":"item.completed","item":{"type":"command_execution"}}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"claims\":[]}"}}"#,
    );
    assert!(
        codex_events(events.as_bytes(), false)
            .body
            .unwrap_err()
            .contains("command_execution")
    );
    assert!(codex_events(events.as_bytes(), true).body.is_ok());
}

#[test]
fn codex_cannot_pass_a_check_after_a_failed_turn() {
    let events = [
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"claims\":[]}"}}"#,
        r#"{"type":"turn.failed","error":{"message":"the API refused the request"}}"#,
    ]
    .join("\n");
    assert_eq!(
        codex_events(events.as_bytes(), false).body.unwrap_err(),
        "the API refused the request"
    );
}

#[test]
fn a_codex_session_runs_the_ruling_check_with_the_codex_protocol() {
    const SESSION_FIXTURE: &str = "BUILDS_TEST_CODEX_REVIEW_SESSION";
    if std::env::var_os(SESSION_FIXTURE).is_none() {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "review::provider::tests::a_codex_session_runs_the_ruling_check_with_the_codex_protocol"])
            .env(SESSION_FIXTURE, "1")
            .env(crate::ack::SESSION, "")
            .env_remove("CODEX_SESSION_ID")
            .env("CODEX_THREAD_ID", "thread-id")
            .env_remove("BUILDS_REVIEWER")
            .output().unwrap();
        let stdout = String::from_utf8(child.stdout).unwrap();
        assert!(
            child.status.success() && stdout.contains("running 1 test"),
            "{stdout}"
        );
        return;
    }
    let scratch = crate::testing::Scratch::new("codex-rulings");
    let mut tool = scratch.tool(0);
    let session = crate::ack::agent::Session::from_env(None).unwrap();
    let reviewer = crate::review::Reviewer::installed(&tool.places, &session, None);
    let executable = reviewer.program.executable();
    let script = scratch.script(
        "codex",
        &format!(
            r#"test "$3" = exec || exit 2
grep -q '^Task: rulings' || exit 3
cat <<'EVENTS'
{{"type":"item.completed","item":{{"type":"agent_message","text":"{{\"claims\":[]}}"}}}}
{COMPLETED}
EVENTS"#
        ),
    );
    tool.reviewer.program = reviewer
        .program
        .with_executable(scratch.dir.join(executable));
    assert_eq!(tool.reviewer.program.executable(), script);
    let store = crate::lists::fixtures::make_store(&tool.places, &["demo"]);
    let details = scratch.dir.join("wait.md");
    std::fs::write(&details, "The agent is waiting for this action.").unwrap();

    let result = crate::lists::question_command::run(
        &tool,
        &crate::lists::item_fixtures::words(&[
            "ask",
            "--title",
            "Resolve a review wait",
            "--kind",
            "your-step",
            "--effect",
            "slows-agents",
            "--details-file",
            details.to_str().unwrap(),
            "--blocks-nothing",
            "The disposable project has no work item.",
        ]),
        &scratch.dir,
        &|_| panic!("the ruling check does not call GitHub"),
    )
    .unwrap();

    assert!(result.contains("asked question"));
    assert_eq!(store.load().unwrap().questions.len(), 1);
}

#[test]
fn an_invalid_event_cannot_change_an_answer_or_erase_reported_usage() {
    let mut events = format!("{COMPLETED}\n").into_bytes();
    events.extend_from_slice(br#"{"type":"item.completed","item":{"type":"agent_message","text":"#);
    events.push(0xff);
    events.extend_from_slice(br#""}}"#);
    let called = codex_events(&events, false);
    assert!(called.body.unwrap_err().contains("invalid event"));
    assert_eq!(called.usage.unwrap().input, 31);
}

#[test]
fn a_codex_failure_reports_the_cli_diagnostic() {
    let scratch = crate::testing::Scratch::new("codex-option");
    let executable = scratch.script("codex", "echo 'unsupported option' >&2\nexit 2");
    let called = invoke(
        &Program::new(executable, scratch.dir.clone()),
        &Selection::Tier(Tier::Medium),
        &json!({}),
        "",
        "check this",
        None,
    );
    assert!(called.body.unwrap_err().contains("unsupported option"));
    assert!(called.usage.tokens.is_none());
}

#[test]
fn the_same_codex_model_at_different_reasoning_efforts_has_a_distinct_setup() {
    let program = Program::new(PathBuf::from("codex"), PathBuf::from("/temp"));
    let medium = Selection::Tier(Tier::Medium);
    let fast = Selection::Tier(Tier::Fast);
    assert_eq!(program.model_name(&medium), program.model_name(&fast));
    assert_ne!(
        program.setup(&medium, "prompt".into()),
        program.setup(&fast, "prompt".into())
    );
}

fn expecting_model(scratch: &crate::testing::Scratch, answer: Value) -> Program {
    expecting_effort(scratch, &[("*", answer)])
}

fn expecting_effort(scratch: &crate::testing::Scratch, answers: &[(&str, Value)]) -> Program {
    let mut shell = String::from(
        r#"set -e
cat >/dev/null
while [ "$#" -gt 1 ]; do
    if [ "$1" = --model ]; then test "$2" = gpt-6.2-sol; model_seen=yes; fi
    case "$2" in
        'model_reasoning_effort="high"') effort=high ;;
        'model_reasoning_effort="medium"') effort=medium ;;
        'model_reasoning_effort="low"') effort=low ;;
    esac
    shift
done
test "$model_seen" = yes
case "$effort" in
"#,
    );
    for (index, (effort, answer)) in answers.iter().enumerate() {
        let event = json!({
            "type": "item.completed",
            "item": {"type": "agent_message", "text": answer.to_string()}
        });
        shell.push_str(&format!(
            "    {effort}) cat <<'ANSWER_{index}'\n{event}\nANSWER_{index}\n        ;;\n"
        ));
    }
    shell.push_str(&format!(
        "    *) exit 2 ;;\nesac\ncat <<'DONE'\n{COMPLETED}\nDONE"
    ));
    Program::new(scratch.script("codex", &shell), scratch.dir.clone())
}

#[test]
fn code_design_capture_and_replay_use_sol() {
    use crate::lists::design_paths::Globs;
    use crate::lists::fixtures::{NOON, stamp};
    use crate::lists::item_fixtures::demo;
    use crate::lists::project::{GlobKind, parse_globs, set_design_globs};
    use crate::review::{Inputs, cases, fakes, review, run};

    let demo = demo("codex-review-models");
    let fake = fakes::fake(&demo.scratch.dir);
    fakes::looks_group(&fake);
    demo.store
        .write(&stamp(NOON), "set design files", |db| {
            set_design_globs(
                db,
                GlobKind::Place,
                &parse_globs("design-places", "demo:design/*.html")?,
            )?;
            set_design_globs(
                db,
                GlobKind::Spec,
                &parse_globs("spec-files", "demo:spec/*.md")?,
            )
        })
        .unwrap();
    let code = demo.add_side(1, "agent", "code");
    demo.commit(&code, "a.rs");
    let design = demo.add_side(2, "agent", "design");
    std::fs::create_dir_all(design.join("design")).unwrap();
    demo.commit(&design, "design/panel.html");
    let branch = |worktree: &Path, label: &str, program: &Program| {
        review(&Inputs {
            store: &demo.store,
            project: "demo",
            repo: "demo",
            worktree,
            base: "origin/main",
            head: None,
            key: label,
            root: &fake.root,
            reviewer: program,
            stamp: &stamp(NOON),
            brief: None,
            runner: None,
        })
        .unwrap()
    };
    let answer = |rule| {
        json!({
            "rules": [{"rule": rule, "verdict": "none", "findings": []}],
            "summary": null,
            "closed": null,
            "sites": null,
        })
    };
    let sol = expecting_model(&demo.scratch, answer("rule-a"));
    let reviewed = branch(&code, "branch:code", &sol);
    assert!(reviewed.refusal.is_none(), "{}", reviewed.text);
    assert!(reviewed.text.contains("gpt-6.2-sol"));

    let sol = expecting_model(&demo.scratch, answer("rule-d"));
    let reviewed = branch(&design, "branch:design", &sol);
    assert!(reviewed.refusal.is_none(), "{}", reviewed.text);
    assert!(reviewed.text.contains("gpt-6.2-sol"));

    let retry = demo.add_side(3, "agent", "retry");
    std::fs::create_dir_all(retry.join("design")).unwrap();
    demo.commit(&retry, "design/panel.html");
    let executable = demo.scratch.script("codex", "cat >/dev/null\nexit 2");
    let offline = Program::new(executable, demo.scratch.dir.clone());
    assert!(branch(&retry, "branch:retry", &offline).refusal.is_some());
    demo.commit(&retry, "a.rs");
    let online = expecting_effort(
        &demo.scratch,
        &[("medium", answer("rule-d")), ("high", answer("rule-a"))],
    );
    let retries = branch(&retry, "branch:retry", &online);
    assert!(retries.refusal.is_none(), "{}", retries.text);
    assert!(retries.text.contains("gpt-6.2-sol"), "{}", retries.text);
    assert!(retries.text.contains("gpt-6.2-sol"), "{}", retries.text);

    let capture = demo.add_side(5, "agent", "capture-code");
    std::fs::write(capture.join(".app-build"), "shots-declared mac app/shots\n").unwrap();
    std::fs::create_dir_all(capture.join("app/shots")).unwrap();
    demo.commit(&capture, "app/shots/AccessibilitySnapshot.swift");
    fakes::capture_rule_file(
        &fake.root,
        "beta",
        "rule-b",
        "app/shots/**",
        "Write the accessibility snapshot.",
    );
    let sol = expecting_model(&demo.scratch, answer("rule-b"));
    let reviewed = branch(&capture, "branch:capture-code", &sol);
    assert!(reviewed.refusal.is_none(), "{}", reviewed.text);
    assert!(reviewed.text.contains("gpt-6.2-sol"));

    let mixed = demo.add_side(6, "agent", "mixed-code-and-board");
    std::fs::create_dir_all(mixed.join("design")).unwrap();
    demo.commit(&mixed, "a.rs");
    demo.commit(&mixed, "design/panel.html");
    let reviewers = expecting_effort(
        &demo.scratch,
        &[("medium", answer("rule-d")), ("high", answer("rule-a"))],
    );
    let reviewed = branch(&mixed, "branch:mixed-code-and-board", &reviewers);
    assert!(reviewed.refusal.is_none(), "{}", reviewed.text);
    assert!(reviewed.text.contains("gpt-6.2-sol"));

    let code_fix = demo.add_side(4, "agent", "review-code-finding");
    demo.commit(&code_fix, "a.rs");
    let mut finding = answer("rule-a");
    finding["rules"][0]["verdict"] = json!("finding");
    finding["rules"][0]["findings"] = json!([{
        "clause": "Never do the bad thing here.", "file": "a.rs", "line": 1,
        "code": "a.rs", "problem": "The code does the bad thing.",
    }]);
    let raises = expecting_model(&demo.scratch, finding);
    let first = branch(&code_fix, "branch:review-code-finding", &raises);
    let landing = first.landing.unwrap();
    let open = demo
        .store
        .write(&stamp(NOON), "settle code finding", |db| {
            let findings = crate::review::record::findings_of_landing(db, landing)?;
            let id = findings.iter().find(|finding| finding.counts()).unwrap().id;
            crate::review::record::settle(
                db,
                id,
                "confirmed",
                "It is a code finding.",
                NOON,
                "judge",
            )?;
            Ok(id)
        })
        .unwrap();
    std::fs::create_dir_all(code_fix.join("spec")).unwrap();
    demo.commit(&code_fix, "spec/app.md");
    let mut closes = answer("rule-a");
    closes["closed"] = json!([open]);
    let closes_reviewer = expecting_model(&demo.scratch, closes);
    let rereviewed = branch(&code_fix, "branch:review-code-finding", &closes_reviewer);
    assert!(rereviewed.refusal.is_none(), "{}", rereviewed.text);
    assert!(rereviewed.text.contains("gpt-6.2-sol"));

    let sol = expecting_model(&demo.scratch, answer("rule-a"));
    let case = cases::Case {
        name: "example".into(),
        project: "demo".into(),
        repo: "demo".into(),
        patch: run::diff_of(&code, "origin/main").unwrap().diff.text,
        clean_for: vec!["rule-a".into()],
        expected: Vec::new(),
        labels: Vec::new(),
    };
    let material = run::Material::load(&fake.root, "demo").unwrap();
    let scope = run::Scope {
        material: &material,
        files: &Globs::default(),
    };
    let replayed = cases::replay_case(scope, &sol, &case, None);
    assert_eq!(replayed.calls[0].model, "gpt-6.2-sol");
    assert!(replayed.calls[0].failure.is_none());
    assert!(cases::totals(&[replayed]).usage.cost_microdollars.is_none());
}

#[test]
fn a_codex_spec_check_uses_sol_even_when_the_rule_uses_the_high_tier() {
    use crate::lists::spec_check::{Brief, Setup, Target, check_all};
    let scratch = crate::testing::Scratch::new("codex-spec-model");
    std::fs::write(
        scratch.dir.join("spec.md"),
        "The app keeps the selected song.",
    )
    .unwrap();
    let sol = expecting_model(&scratch, json!({"conflicts": [], "unstated": []}));
    let checked = check_all(
        &sol,
        &Setup {
            model: Selection::Tier(Tier::High),
            rule_text: "Every change agrees with its specification.".into(),
            rules_version: "example".into(),
        },
        &Brief {
            name: "brief.md".into(),
            text: "Keep the selected song.".into(),
        },
        &[Target {
            repo: "demo".into(),
            worktree: scratch.dir.clone(),
            head: "example".into(),
            spec_files: vec!["spec.md".into()],
        }],
    )
    .remove(0);
    assert_eq!(checked.model, "gpt-6.2-sol");
    assert!(checked.failure.is_none(), "{:?}", checked.failure);
}

#[test]
fn claude_records_the_model_that_answered_instead_of_its_alias() {
    let reply = serde_json::json!({
        "is_error": false,
        "usage": {"input_tokens": 0, "output_tokens": 0},
        "structured_output": {"rules": []},
        "modelUsage": {
            "claude-opus-4-8": {},
            "claude-haiku-4-5": {},
        },
    });
    let invoked = claude_envelope(reply.to_string().as_bytes(), "opus");
    assert_eq!(invoked.model, "claude-opus-4-8");
}

#[test]
fn claude_refuses_invalid_envelope_fields_and_token_overflow() {
    for (field, value, expected) in [
        ("is_error", json!("false"), "invalid is_error"),
        (
            "structured_output",
            json!(null),
            "invalid structured_output",
        ),
        ("usage", json!({"input_tokens": -1}), "invalid input_tokens"),
        (
            "usage",
            json!({"output_tokens": "7"}),
            "invalid output_tokens",
        ),
        (
            "usage",
            json!({"input_tokens": i64::MAX, "output_tokens": 0, "cache_read_input_tokens": 1, "cache_creation_input_tokens": 0}),
            "supported range",
        ),
    ] {
        let mut envelope = json!({"is_error": false, "result": "{\"rules\":[]}"});
        envelope[field] = value;
        let response = claude_envelope(envelope.to_string().as_bytes(), "opus");
        assert!(response.body.unwrap_err().contains(expected));
    }
}

#[test]
fn a_failed_claude_process_keeps_reported_usage() {
    let scratch = crate::testing::Scratch::new("claude-failed-envelope");
    let executable = scratch.script("claude", r#"cat >/dev/null
cat <<'REPLY'
{"is_error":true,"result":"rate limit","total_cost_usd":0,"usage":{"input_tokens":5,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"modelUsage":{"claude-opus-4-8":{}}}
REPLY
exit 2"#);
    let called = invoke(
        &Program::new(executable, scratch.dir.clone()),
        &Selection::Tier(Tier::High),
        &json!({}),
        "",
        "check",
        None,
    );
    assert_eq!(called.usage.tokens.unwrap().input, 5);
    assert_eq!(called.usage.cost_microdollars, Some(0));
    assert_eq!(called.model, "claude-opus-4-8");
    assert!(
        called
            .body
            .unwrap_err()
            .contains("exited with exit status: 2")
    );
}

#[test]
fn codex_reports_stdout_when_the_process_fails_without_stderr() {
    let scratch = crate::testing::Scratch::new("codex-failed-stdout");
    let executable = scratch.script("codex", "cat >/dev/null\necho 'credential failed'\nexit 2");
    let called = invoke(
        &Program::new(executable, scratch.dir.clone()),
        &Selection::Tier(Tier::High),
        &json!({}),
        "",
        "check",
        None,
    );
    assert!(called.body.unwrap_err().contains("credential failed"));
    assert_eq!(called.model, "unknown (requested gpt-6.2-sol)");
}

#[test]
fn codex_startup_errors_do_not_claim_a_model_ran() {
    let scratch = crate::testing::Scratch::new("codex-startup-failure");
    let called = invoke(
        &Program::new(scratch.dir.join("codex"), scratch.dir.clone()),
        &Selection::Tier(Tier::High),
        &json!({}),
        "",
        "check",
        None,
    );
    assert!(called.body.unwrap_err().contains("running"));
    assert_eq!(called.model, "unknown (requested gpt-6.2-sol)");
}

#[test]
fn a_claude_error_keeps_its_reported_price_when_tokens_are_invalid() {
    let reply = json!({
        "is_error": true,
        "result": "rate limited",
        "total_cost_usd": 0.01,
        "usage": {"input_tokens": -1, "output_tokens": 0},
    });
    let called = claude_envelope(reply.to_string().as_bytes(), "opus");
    assert!(called.usage.tokens.is_none());
    assert_eq!(called.usage.cost_microdollars, Some(10_000));
    let diagnostic = called.body.unwrap_err();
    assert!(diagnostic.contains("rate limited") && diagnostic.contains("invalid input_tokens"));
}

#[test]
fn an_unreported_output_count_leaves_the_answer_and_records_no_tokens() {
    let reply = json!({
        "is_error": false,
        "structured_output": {"claims": []},
        "usage": {"input_tokens": 20},
    });
    let called = claude_envelope(reply.to_string().as_bytes(), "opus");
    assert!(called.usage.tokens.is_none());
    assert!(called.body.is_ok());
    assert!(claude_cost(&json!({"total_cost_usd": -0.0000001})).is_err());
}

#[test]
fn invalid_codex_usage_keeps_a_tool_violation_visible() {
    let reply = [
        r#"{"type":"item.completed","item":{"type":"command_execution"}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":-4}}"#,
    ]
    .join("\n");
    let called = codex_events(reply.as_bytes(), false);
    assert!(called.usage.is_none());
    let diagnostic = called.body.unwrap_err();
    assert!(
        diagnostic.contains("command_execution") && diagnostic.contains("invalid input_tokens")
    );
}

#[test]
fn codex_keeps_its_answer_failure_and_usage_after_cleanup_fails() {
    let scratch = crate::testing::Scratch::new("codex-cleanup-diagnostic");
    let shell = format!(
        r#"set -e
cat >/dev/null
mv "$PWD" "$PWD.renamed"
cat <<'EVENTS'
{{"type":"turn.failed","error":{{"message":"the provider could not finish"}}}}
{COMPLETED}
EVENTS"#
    );
    let executable = scratch.script("codex", &shell);
    let response = invoke(
        &Program::new(executable, scratch.dir.clone()),
        &Selection::Tier(Tier::Fast),
        &json!({}),
        "",
        "check",
        None,
    );
    assert_eq!(response.usage.tokens.unwrap().input, 31);
    let why = response.body.unwrap_err();
    assert!(
        why.contains("the provider could not finish") && why.contains("removing"),
        "{why}"
    );
}

#[test]
fn codex_keeps_a_completed_answer_without_a_usage_event() {
    let events = concat!(
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"claims\":[]}"}}"#,
        "\n",
        r#"{"type":"turn.completed"}"#,
    );
    let answer = codex_events(events.as_bytes(), false);
    assert!(answer.completed);
    assert!(answer.usage.is_none());
    assert_eq!(answer.body.unwrap(), json!({"claims": []}));
}

#[test]
fn codex_refuses_cache_usage_above_the_total_input() {
    let usage = json!({"input_tokens": 4, "cached_input_tokens": 5, "cache_write_input_tokens": 2, "output_tokens": 1});
    assert!(
        codex_usage(&usage)
            .unwrap_err()
            .contains("more input tokens")
    );
}
