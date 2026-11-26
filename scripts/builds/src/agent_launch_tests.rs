use super::*;

fn init(model: &str, tools: &[&str], mcp: usize, skills: &[&str]) -> Init {
    Init {
        session: "s1".into(),
        model: model.into(),
        tools: tools.iter().map(|tool| tool.to_string()).collect(),
        mcp_servers: vec![serde_json::json!({"name": "x"}); mcp],
        skills: skills.iter().map(|skill| skill.to_string()).collect(),
    }
}

#[test]
fn a_new_agent_gets_the_trimmed_flags_the_model_the_name_and_a_session_id() {
    assert_eq!(
        arguments(
            "claude-sonnet-5-5",
            "demo 7 implementing",
            Session::New("s1")
        ),
        [
            "-p",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--tools",
            "Bash,Edit,LSP,Monitor,Read,TaskStop,ToolSearch,Write",
            "--dangerously-skip-permissions",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-sonnet-5-5",
            "--name",
            "demo 7 implementing",
            "--session-id",
            "s1",
        ]
    );
}

#[test]
fn a_resumed_agent_gets_the_same_flags_and_resume_in_place_of_a_session_id() {
    let new = arguments("m", "n", Session::New("s1"));
    let resumed = arguments("m", "n", Session::Resume("s1"));
    assert_eq!(new[..new.len() - 2], resumed[..resumed.len() - 2]);
    assert_eq!(resumed[resumed.len() - 2..], ["--resume", "s1"]);
}

#[test]
fn the_command_clears_the_callers_identity_and_the_claude_code_variables() {
    let command = command(Path::new("claude"), &[], Path::new("/w"));
    let cleared: Vec<String> = command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for name in [
        "CLAUDE_CODE_SESSION_ID",
        "BUILDS_AGENT",
        "BUILDS_ACKNOWLEDGED",
        "CLAUDE_CODE_MESSAGING_SOCKET",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        "CODEX_SESSION_ID",
        "CODEX_THREAD_ID",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
    ] {
        assert!(cleared.iter().any(|cleared| cleared == name), "{name}");
    }
    assert_eq!(command.get_current_dir(), Some(Path::new("/w")));
}

#[test]
fn the_launch_check_passes_the_eight_tools_in_any_order_and_no_server_or_skill() {
    let mut tools = TOOLS.to_vec();
    tools.reverse();
    check_init("m", &init("m", &tools, 0, &[])).unwrap();
}

#[test]
fn the_launch_check_fails_for_an_mcp_server_a_skill_a_ninth_tool_or_another_model() {
    let mut nine = TOOLS.to_vec();
    nine.push("Skill");
    let cases = [
        (init("m", &TOOLS, 1, &[]), "1 MCP servers"),
        (
            init("m", &TOOLS, 0, &["publish-designs"]),
            "publish-designs",
        ),
        (init("m", &nine, 0, &[]), "Skill"),
        (init("other", &TOOLS, 0, &[]), "it runs other"),
        (init("m", &TOOLS[..7], 0, &[]), "its tools are"),
    ];
    for (init, said) in cases {
        let error = check_init("m", &init).unwrap_err().text();
        assert!(error.contains(said), "{said}: {error}");
        assert!(error.contains("builds stopped the agent"), "{error}");
    }
}

#[test]
fn an_agents_log_is_the_log_file_of_a_session_folder_of_the_agents_folder() {
    assert!(is_log(Path::new("/Volumes/Builds/agents/s1/log.jsonl")));
    assert!(!is_log(Path::new(
        "/Volumes/Builds/agents/s1/scratch/log.jsonl"
    )));
    assert!(!is_log(Path::new("/Volumes/Builds/other/s1/log.jsonl")));
    assert!(!is_log(Path::new("/Volumes/Builds/agents/s1/other.jsonl")));
}

#[test]
fn run_and_resume_start_an_agent_and_nothing_else_does() {
    assert!(starts_agent(&["agent", "run", "--role", "planner"]));
    assert!(starts_agent(&["agent", "resume", "s1"]));
    assert!(!starts_agent(&["agent", "list"]));
    assert!(!starts_agent(&["agent", "start"]));
    assert!(!starts_agent(&["item", "take"]));
}
