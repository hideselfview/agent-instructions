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
    let called = codex_events(events.as_bytes(), "gpt-6.2-sol", false);
    assert_eq!(called.model, "gpt-6.2-sol");
    assert_eq!(called.body.unwrap(), json!({"claims": []}));
    assert_eq!(
        called.usage,
        Usage {
            tokens_in: 31,
            tokens_out: 12,
            tokens_cached: 7,
            tokens_written: 2,
            cost_microdollars: None,
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
        codex_events(events.as_bytes(), "gpt-6.2-sol", false)
            .body
            .unwrap_err()
            .contains("command_execution")
    );
    assert!(
        codex_events(events.as_bytes(), "gpt-6.2-sol", true)
            .body
            .is_ok()
    );
}

#[test]
fn codex_cannot_pass_a_check_after_a_failed_turn() {
    let events = [
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"claims\":[]}"}}"#,
        r#"{"type":"turn.failed","error":{"message":"the API refused the request"}}"#,
    ]
    .join("\n");
    assert_eq!(
        codex_events(events.as_bytes(), "gpt-6.2-sol", false)
            .body
            .unwrap_err(),
        "the API refused the request"
    );
}

#[test]
fn a_codex_session_runs_the_ruling_check_with_the_codex_protocol() {
    let scratch = crate::testing::Scratch::new("codex-rulings");
    let mut tool = scratch.tool(0);
    let executable = default_program(None, Some(""), true);
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
    tool.reviewer.program = Program::new(scratch.dir.join(executable), scratch.dir.clone());
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
    let called = codex_events(&events, "gpt-6.2-sol", false);
    assert!(called.body.unwrap_err().contains("invalid event"));
    assert_eq!(called.usage.tokens_in, 31);
    assert!(called.usage.cost_microdollars.is_none());
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

fn expecting_model(scratch: &crate::testing::Scratch, name: &str, answer: Value) -> Program {
    let event = json!({
        "type": "item.completed",
        "item": {"type": "agent_message", "text": answer.to_string()}
    });
    let shell = format!(
        r#"set -e
cat > /dev/null
while [ "$#" -gt 0 ]; do
    if [ "$1" = --model ]; then
        test "$2" = "{name}"
        break
    fi
    shift
done
test "$#" -gt 0
cat <<'EVENTS'
{event}
{COMPLETED}
EVENTS"#
    );
    Program::new(scratch.script("codex", &shell), scratch.dir.clone())
}

#[test]
fn actual_code_review_uses_astra_and_design_and_replay_use_sol() {
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
    let answer = |rule| json!({"rules": [{"rule": rule, "verdict": "none", "findings": []}]});
    let astra = expecting_model(&demo.scratch, "gpt-6.2-astra", answer("rule-a"));
    let reviewed = branch(&code, "branch:code", &astra);
    assert!(reviewed.refusal.is_none(), "{}", reviewed.text);
    assert!(reviewed.text.contains("gpt-6.2-astra"));

    let sol = expecting_model(&demo.scratch, "gpt-6.2-sol", answer("rule-d"));
    let reviewed = branch(&design, "branch:design", &sol);
    assert!(reviewed.refusal.is_none(), "{}", reviewed.text);
    assert!(reviewed.text.contains("gpt-6.2-sol"));

    let sol = expecting_model(&demo.scratch, "gpt-6.2-sol", answer("rule-a"));
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
    assert!(cases::totals(&[replayed]).cost_microdollars.is_none());
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
    let sol = expecting_model(
        &scratch,
        "gpt-6.2-sol",
        json!({"conflicts": [], "unstated": []}),
    );
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
