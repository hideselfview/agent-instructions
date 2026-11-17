use super::*;
use crate::review::rules::Rule;

fn group(name: &str, enumerate: bool) -> Group {
    Group {
        name: name.to_string(),
        question: format!("question of {name}"),
        model: "sonnet".to_string(),
        dir: String::new(),
        rules: vec!["r".to_string()],
        instructions: None,
        project_facts: false,
        enumerate,
        sites: enumerate.then(|| "handles rows a query returned".to_string()),
        commit_text: false,
        passes: 1,
    }
}

fn parts_of(group: &Group, rule: &Rule, diff: &str) -> Parts {
    build(&Input {
        group,
        rules: &[rule],
        project_text: "Project facts.\n",
        brief: Some(&("plans/b.md".to_string(), "The brief.\n".to_string())),
        diff,
        context: "7: let x = 1;\n",
        open: &[],
    })
}

#[test]
fn the_system_prompt_holds_the_shared_part_and_the_message_holds_the_change() {
    let rule_a = Rule::parse("a", "---\npaths: ['**']\n---\nRule A body.\n");
    let rule_b = Rule::parse("b", "---\npaths: ['**']\n---\nRule B body.\n");
    let first = parts_of(
        &group("one", false),
        &rule_a,
        "diff --git a/a.rs b/a.rs\n+x\n",
    );
    let second = parts_of(
        &group("two", true),
        &rule_b,
        "diff --git a/a.rs b/a.rs\n+x\n",
    );
    for part in [
        "Project facts.",
        "The brief.",
        "Rule A body.",
        "How to answer",
    ] {
        assert!(first.system.contains(part), "{part}");
    }
    assert!(!first.system.contains("+x") && !first.system.contains("let x = 1"));
    assert!(first.system.contains("open it before judging"));
    assert!(first.message.contains("+x") && first.message.contains("7: let x = 1;"));
    assert!(!first.message.contains("Rule A body.") && !first.message.contains("Project facts."));
    // Two calls of one group differ only in the message.
    let again = parts_of(
        &group("one", false),
        &rule_a,
        "diff --git a/b.rs b/b.rs\n+y\n",
    );
    assert_eq!(first.system, again.system);
    assert_ne!(first.message, again.message);
    assert!(
        first.system.contains("Put your answer in `rules`")
            && second
                .system
                .contains("Put your answer in `summary` and `sites`")
    );
    assert!(
        second
            .system
            .contains("every site in the diff that handles rows a query returned")
    );
}

#[test]
fn every_group_gets_the_same_schema() {
    assert_eq!(schema(), schema());
    assert!(
        schema()["properties"]["sites"].is_object() && schema()["properties"]["rules"].is_object()
    );
}

#[test]
fn the_setup_read_back_from_a_system_prompt_is_the_one_a_call_records() {
    let rule = Rule::parse("a", "---\npaths: ['**']\n---\nRule A body.\n");
    let mut one = group("one", true);
    one.passes = 2;
    one.commit_text = true;
    let recorded = setup(&one, &[&rule]);
    let sent = parts_of(&one, &rule, "diff --git a/a.rs b/a.rs\n+x\n").system;
    assert_eq!(
        setup_of_system(&sent, 2, true).as_deref(),
        Some(recorded.as_str())
    );
    assert!(recorded.contains("Rule A body.") && recorded.contains("passes: 2"));
    assert!(
        !recorded.contains("Project facts.") && !recorded.contains("The brief."),
        "what each review brings is not part of the setup"
    );
    // The rules' text, the group's settings and its passes each change it;
    // the model is recorded beside it.
    let edited = Rule::parse("a", "---\npaths: ['**']\n---\nRule A body, edited.\n");
    assert_ne!(setup(&one, &[&edited]), recorded);
    let mut asked_otherwise = one.clone();
    asked_otherwise.instructions = Some("Look harder.".to_string());
    assert_ne!(setup(&asked_otherwise, &[&rule]), recorded);
    let mut once = one.clone();
    once.passes = 1;
    assert_ne!(setup(&once, &[&rule]), recorded);
    let mut on_haiku = one.clone();
    on_haiku.model = "haiku".to_string();
    assert_eq!(setup(&on_haiku, &[&rule]), recorded);
    assert_eq!(setup_of_system("no group part here", 1, false), None);
}
