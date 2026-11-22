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
    let called = codex_events(&events, "gpt-6.2-sol", false);
    assert_eq!(called.model, "gpt-6.2-sol");
    assert_eq!(called.body.unwrap(), json!({"claims": []}));
    assert_eq!(
        called.usage,
        Usage {
            tokens_in: 31,
            tokens_out: 12,
            tokens_cached: 7,
            tokens_written: 2,
            cost_microdollars: 0,
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
        codex_events(&events, "gpt-6.2-sol", false)
            .body
            .unwrap_err()
            .contains("command_execution")
    );
    assert!(codex_events(&events, "gpt-6.2-sol", true).body.is_ok());
}

#[test]
fn codex_cannot_pass_a_check_after_a_failed_turn() {
    let events = [
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"claims\":[]}"}}"#,
        r#"{"type":"turn.failed","error":{"message":"the API refused the request"}}"#,
    ]
    .join("\n");
    assert_eq!(
        codex_events(&events, "gpt-6.2-sol", false)
            .body
            .unwrap_err(),
        "the API refused the request"
    );
}
