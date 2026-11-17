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
        r#"{"session_id": "s1", "hook_event_name": "SubagentStart", "agent_id": "a91fe7c601b214585", "agent_type": "general-purpose"}"#,
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
            r#"{"session_id": "s1", "hook_event_name": "SessionStart", "source": "compact"}"#
        )
        .is_err()
    );
    assert!(
        start(
            &places,
            r#"{"session_id": "s1", "hook_event_name": "PreToolUse"}"#
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
