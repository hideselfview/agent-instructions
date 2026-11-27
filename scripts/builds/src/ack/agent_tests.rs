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
            crate::run_command(
                &[command.to_string()],
                session,
                &crate::testing::no_system()
            ),
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

#[test]
fn codex_workers_identify_themselves_by_thread_and_cannot_claim_the_coordinator() {
    let session = "00000000-0000-4000-8000-000000000001";
    let worker = "00000000-0000-4000-8000-000000000002";
    let main = Identity::codex(Some(session), Some(session), None)
        .unwrap()
        .caller();
    assert!(!main.is_dispatched());
    let child = Identity::codex(Some(session), Some(worker), Some(worker))
        .unwrap()
        .caller();
    assert_eq!(child.session, session);
    assert_eq!(child.id, worker);
    assert!(child.is_dispatched());
    assert!(Identity::codex(Some(session), Some(worker), Some(session)).is_err());
    assert!(Identity::codex(Some(session), None, None).is_err());
    assert!(Identity::codex(None, Some(worker), None).is_err());
}

#[test]
fn a_claude_session_holds_the_inbox_its_shell_names_or_why_it_cannot() {
    let claude = || Session::of(Some("s1".into()), None, None, Some("s1".into()));
    let inbox = |session: Session| match session {
        Session::Claude { inbox, .. } => inbox,
        other => panic!("{other:?}"),
    };
    assert_eq!(inbox(claude().with_inbox(None, None)), None);
    assert_eq!(inbox(claude().with_inbox(Some(" ".into()), None)), None);
    let held = inbox(claude().with_inbox(Some("/tmp/s.sock".into()), Some("t".into())));
    assert_eq!(
        held.unwrap().unwrap().stored(),
        ("claude", "/tmp/s.sock", Some("t"))
    );
    let relative = inbox(claude().with_inbox(Some("s.sock".into()), None));
    assert!(relative.unwrap().unwrap_err().contains("absolute"));
    // A terminal has no inbox to hold.
    assert_eq!(
        Session::Terminal.with_inbox(Some("/tmp/s.sock".into()), None),
        Session::Terminal
    );
}

#[test]
fn a_failed_recording_of_rules_records_none_of_them() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("agent-rules-failed");
    let places = scratch.places();
    register(&places, "s1", "a1").unwrap();
    give_rules(&places, "s1", "a1", &["plain".to_string()]).unwrap();
    let folder = rules_file(&places, "s1", "a1")
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let slugs = ["one".to_string(), "two".to_string()];
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let failed = give_rules(&places, "s1", "a1", &slugs);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(failed.is_err());
    assert!(was_given_rule(&places, "s1", "a1", "plain").unwrap());
    for slug in &slugs {
        assert!(!was_given_rule(&places, "s1", "a1", slug).unwrap());
    }
    give_rules(&places, "s1", "a1", &slugs).unwrap();
    for slug in ["plain", "one", "two"] {
        assert!(was_given_rule(&places, "s1", "a1", slug).unwrap());
    }
    forget_rules(&places, "s1", "a1").unwrap();
    assert!(!was_given_rule(&places, "s1", "a1", "plain").unwrap());
}
