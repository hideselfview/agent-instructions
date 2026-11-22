use super::*;
use crate::testing::Scratch;

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

#[test]
fn a_subagents_start_registers_its_id_and_tells_it_the_id() {
    let scratch = Scratch::new("agent-start");
    let places = scratch.places();
    let told = start(
        &places,
        HookInput::parse(r#"{"session_id": "s1", "hook_event_name": "SubagentStart", "agent_id": "a91fe7c601b214585", "agent_type": "general-purpose"}"#).unwrap(),
    )
    .unwrap();
    let output: serde_json::Value = serde_json::from_str(&told).unwrap();
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "SubagentStart"
    );
    assert!(
        output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .starts_with(
                "Your builds agent id is a91fe7c601b214585. Pass --agent a91fe7c601b214585"
            )
    );
    assert!(is_registered(&places, "s1", "a91fe7c601b214585").unwrap());
    assert!(!is_registered(&places, "s2", "a91fe7c601b214585").unwrap());
    assert!(!is_registered(&places, "s1", "s1").unwrap());
}

#[test]
fn a_compaction_or_another_event_is_no_start() {
    let scratch = Scratch::new("agent-start-refused");
    let places = scratch.places();
    assert!(
        start(
            &places,
            HookInput::parse(
                r#"{"session_id": "s1", "hook_event_name": "SessionStart", "source": "compact"}"#
            )
            .unwrap(),
        )
        .is_err()
    );
    assert!(
        start(
            &places,
            HookInput::parse(r#"{"session_id": "s1", "hook_event_name": "PreToolUse"}"#).unwrap(),
        )
        .is_err()
    );
    assert!(!is_registered(&places, "s1", "s1").unwrap());
}

#[test]
fn an_id_that_could_name_another_path_is_refused() {
    let scratch = Scratch::new("agent-bad-id");
    let places = scratch.places();
    assert!(register(&places, "s1", "../x").is_err());
    assert!(is_registered(&places, "", "a1").is_err());
}

#[test]
fn the_agent_flag_leaves_the_arguments_and_stops_at_the_end_of_the_flags() {
    let (rest, given) = split_flag(&strings(&[
        "review", "--agent", "a1", "side-1", "--", "--agent", "x",
    ]))
    .unwrap();
    assert_eq!(rest, strings(&["review", "side-1", "--", "--agent", "x"]));
    assert_eq!(given.as_deref(), Some("a1"));
    assert!(split_flag(&strings(&["review", "--agent"])).is_err());
    assert!(split_flag(&strings(&["review", "--agent", "a1", "--agent", "a1"])).is_err());
}

#[test]
fn builds_agent_names_the_agent_as_the_flag_does() {
    assert_eq!(chosen(Some("a1"), None).unwrap().as_deref(), Some("a1"));
    assert_eq!(chosen(None, Some("a1")).unwrap().as_deref(), Some("a1"));
    assert_eq!(
        chosen(Some("a1"), Some("a1")).unwrap().as_deref(),
        Some("a1")
    );
    assert_eq!(chosen(None, None).unwrap(), None);
    assert!(chosen(Some("a1"), Some("a2")).is_err());
}

#[test]
fn a_lists_folder_with_no_parent_has_no_place_for_the_agents() {
    let scratch = Scratch::new("agent-no-parent");
    let mut places = scratch.places();
    places.lists = std::path::PathBuf::from("/");
    assert!(register(&places, "s1", "a1").is_err());
}

#[test]
fn a_caller_is_a_session_and_an_agent_and_the_person_has_no_session() {
    assert_eq!(
        Session::of(None, None, None, Some("a1".into()))
            .caller()
            .unwrap(),
        None
    );
    assert_eq!(
        Session::of(Some("".into()), None, None, None)
            .caller()
            .unwrap(),
        None
    );
    assert!(
        Session::of(Some("s1".into()), None, None, None)
            .caller()
            .unwrap_err()
            .text()
            .contains("--agent")
    );
    let main = Session::of(Some("s1".into()), None, None, Some("s1".into()))
        .caller()
        .unwrap()
        .unwrap();
    assert!(!main.is_dispatched());
    let dispatched = Session::of(Some("s1".into()), None, None, Some("a1".into()))
        .caller()
        .unwrap()
        .unwrap();
    assert!(dispatched.is_dispatched());
}

#[test]
fn help_and_version_work_with_a_malformed_claude_session_id() {
    use std::os::unix::ffi::OsStringExt;
    use std::process::ExitCode;

    for command in ["help", "version"] {
        let invalid = std::ffi::OsString::from_vec(vec![0xff]);
        let session = Session::of(Some(invalid), None, None, None);
        assert!(
            session
                .caller()
                .unwrap_err()
                .text()
                .contains("not a UTF-8 session ID")
        );
        assert_eq!(
            crate::run_command(&[command.to_string()], session),
            ExitCode::SUCCESS
        );
    }
}

#[test]
fn a_running_command_line_loses_its_agent_option_wherever_it_stands() {
    let words = |line: &str| line.split(' ').map(str::to_string).collect::<Vec<_>>();
    assert_eq!(
        without_flag(&words("builds --agent a1 land . --part 3")),
        ["builds", "land", ".", "--part", "3"]
    );
    assert_eq!(
        without_flag(&words("builds land . --agent a1")),
        ["builds", "land", "."]
    );
    assert_eq!(
        without_flag(&words("builds cargo test -- --agent a1")),
        ["builds", "cargo", "test", "--", "--agent", "a1"]
    );
}
