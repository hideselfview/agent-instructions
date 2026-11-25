use super::*;
use crate::lists::fixtures::make_store;
use crate::review::frequent::Quoted;
use crate::testing::{Scratch, repo};

const RULE: &str =
    "---\npaths: ['**/*.rs']\n---\n\n## Write plainly\n\nName the actor and the action.\n";

fn rules_root(scratch: &Scratch) -> PathBuf {
    let root = scratch.dir.join("rules-root");
    std::fs::create_dir_all(root.join("rules/text")).unwrap();
    std::fs::write(root.join("rules/text/plain.md"), RULE).unwrap();
    std::fs::write(
        root.join("rules/text/swift.md"),
        "---\npaths: ['**/*.swift']\n---\n\nSwift only.\n",
    )
    .unwrap();
    root
}

fn edit(agent: Option<&str>, file: &Path) -> EditInput {
    let agent = agent.map_or(String::new(), |id| format!(r#""agent_id": "{id}","#));
    serde_json::from_str(&format!(
        r#"{{"session_id": "s1", "hook_event_name": "PreToolUse", {agent} "tool_name": "Edit", "tool_input": {{"file_path": {:?}}}}}"#,
        file.display().to_string()
    ))
    .unwrap()
}

fn context(output: &str) -> String {
    let output: serde_json::Value = serde_json::from_str(output).unwrap();
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn an_agent_gets_each_rule_that_covers_a_file_once_on_its_first_edit() {
    let scratch = Scratch::new("rules-on-edit-once");
    let places = scratch.places();
    let root = rules_root(&scratch);
    let work = scratch.dir.join("work");
    repo(&work, "target 1\n");
    make_store(&places, &["work"]);
    let first = on_edit(&places, &root, &edit(Some("a1"), &work.join("src/lib.rs")))
        .unwrap()
        .expect("the first edit gets the rule");
    let text = context(&first);
    assert!(text.contains("### `rules/text/plain.md`"), "{text}");
    assert!(text.contains("Name the actor and the action."), "{text}");
    assert!(!text.contains("Swift only."), "{text}");
    let again = on_edit(
        &places,
        &root,
        &edit(Some("a1"), &work.join("src/other.rs")),
    )
    .unwrap();
    assert_eq!(again, None);
    let other_agent = on_edit(&places, &root, &edit(Some("a2"), &work.join("src/lib.rs"))).unwrap();
    assert!(other_agent.is_some());
    let swift = on_edit(&places, &root, &edit(Some("a1"), &work.join("App.swift")))
        .unwrap()
        .expect("a file another rule covers gets that rule");
    assert!(context(&swift).contains("Swift only."));
}

#[test]
fn a_compacted_agent_gets_its_rules_again() {
    let scratch = Scratch::new("rules-on-edit-compacted");
    let places = scratch.places();
    let root = rules_root(&scratch);
    let work = scratch.dir.join("work");
    repo(&work, "target 1\n");
    make_store(&places, &["work"]);
    let input = edit(None, &work.join("src/lib.rs"));
    assert!(on_edit(&places, &root, &input).unwrap().is_some());
    assert_eq!(on_edit(&places, &root, &input).unwrap(), None);
    agent::forget_rules(&places, "s1", "s1").unwrap();
    assert!(on_edit(&places, &root, &input).unwrap().is_some());
}

#[test]
fn an_edit_of_a_file_in_no_repo_or_of_no_file_gets_nothing() {
    let scratch = Scratch::new("rules-on-edit-none");
    let places = scratch.places();
    let root = rules_root(&scratch);
    let loose = scratch.dir.join("loose");
    std::fs::create_dir_all(&loose).unwrap();
    assert_eq!(
        on_edit(&places, &root, &edit(None, &loose.join("a.rs"))).unwrap(),
        None
    );
    let no_file: EditInput = serde_json::from_str(
        r#"{"session_id": "s1", "hook_event_name": "PreToolUse", "tool_input": {}}"#,
    )
    .unwrap();
    assert_eq!(on_edit(&places, &root, &no_file).unwrap(), None);
}

#[test]
fn another_event_is_no_edit() {
    let scratch = Scratch::new("rules-on-edit-event");
    let input: EditInput = serde_json::from_str(
        r#"{"session_id": "s1", "hook_event_name": "PostToolUse", "tool_input": {"file_path": "/x/a.rs"}}"#,
    )
    .unwrap();
    assert!(on_edit(&scratch.places(), &rules_root(&scratch), &input).is_err());
}

#[test]
fn a_rule_is_followed_by_the_findings_reviews_confirmed_of_it() {
    let rule = Rule::parse("plain", RULE);
    let other = Rule::parse("other", RULE);
    let confirmed = [Frequent {
        rule: "plain".to_string(),
        findings: 3,
        weight: 2.0,
        quoted: vec![Quoted {
            text: "Name the actor and the action.".to_string(),
            findings: 2,
        }],
    }];
    let text = render("src/lib.rs", &[&rule, &other], &confirmed);
    assert!(text.starts_with("Coding rules for `src/lib.rs`."), "{text}");
    let beside = "Name the actor and the action.\n\nRecent first reviews confirmed 3 findings of this rule. The paragraphs they quote most:\n- 2x Name the actor and the action.\n";
    assert!(text.contains(beside), "{text}");
    assert_eq!(text.matches("Recent first reviews").count(), 1, "{text}");
}
