use super::*;

fn init_line(tools: &str, mcp: &str, skills: &str) -> String {
    format!(
        r#"{{"type":"system","subtype":"init","session_id":"s1","model":"claude-sonnet-5-5","tools":{tools},"mcp_servers":{mcp},"skills":{skills}}}"#
    )
}

#[test]
fn the_init_line_gives_the_session_model_tools_servers_and_skills() {
    let line = init_line(r#"["Bash","Read"]"#, "[]", r#"["publish-designs"]"#);
    assert_eq!(
        parse(&line).unwrap(),
        Line::Init(Init {
            session: "s1".into(),
            model: "claude-sonnet-5-5".into(),
            tools: vec!["Bash".into(), "Read".into()],
            mcp_servers: vec![],
            skills: vec!["publish-designs".into()],
        })
    );
}

#[test]
fn an_init_line_without_its_tool_list_fails() {
    let line = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m","mcp_servers":[],"skills":[]}"#;
    assert!(parse(line).is_err());
}

#[test]
fn an_assistant_turn_counts_input_cache_creation_and_cache_read_tokens() {
    let line = r#"{"type":"assistant","message":{"usage":{"input_tokens":2,"cache_creation_input_tokens":422,"cache_read_input_tokens":10225,"output_tokens":16}}}"#;
    assert_eq!(parse(line).unwrap(), Line::Turn { context: 10649 });
}

#[test]
fn a_subagents_turn_is_not_the_agents_context() {
    let line = r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"usage":{"input_tokens":2,"cache_creation_input_tokens":1,"cache_read_input_tokens":1}}}"#;
    assert_eq!(parse(line).unwrap(), Line::Other);
}

#[test]
fn the_result_line_gives_the_report_and_the_models_window() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"Landed.\nChecks passed.","modelUsage":{"claude-sonnet-5-5":{"contextWindow":1000000}}}"#;
    assert_eq!(
        parse(line).unwrap(),
        Line::Result(Report {
            text: "Landed.\nChecks passed.".into(),
            is_error: false,
            window: Some(1_000_000),
        })
    );
}

#[test]
fn other_lines_and_text_that_is_no_json_are_other() {
    assert_eq!(parse(r#"{"type":"user"}"#).unwrap(), Line::Other);
    assert_eq!(parse("warning: something").unwrap(), Line::Other);
}
