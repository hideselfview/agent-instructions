use super::*;
use crate::review::rules::Rule;

fn group(name: &str, enumerate: bool) -> Group {
    Group {
        name: name.to_string(),
        question: format!("question of {name}"),
        model: "sonnet".to_string(),
        rules: vec!["r".to_string()],
        principles: Vec::new(),
        instructions: None,
        project_facts: false,
        enumerate,
    }
}

fn prompt_of(group: &Group, rule: &Rule) -> String {
    build(&Input {
        group,
        rules: &[rule],
        project_text: "Project facts.\n",
        principles: &[],
        brief: Some(&("plans/b.md".to_string(), "The brief.\n".to_string())),
        settled: &[],
        diff: "diff --git a/a.rs b/a.rs\n+x\n",
    })
}

#[test]
fn the_prompts_of_a_landing_share_one_prefix_and_differ_only_after_it() {
    let rule_a = Rule::parse("a", "---\npaths: ['**']\n---\nRule A body.\n");
    let rule_b = Rule::parse("b", "---\npaths: ['**']\n---\nRule B body.\n");
    let first = prompt_of(&group("one", false), &rule_a);
    let second = prompt_of(&group("two", true), &rule_b);
    let shared_text = shared(&Input {
        group: &group("one", false),
        rules: &[&rule_a],
        project_text: "Project facts.\n",
        principles: &[],
        brief: Some(&("plans/b.md".to_string(), "The brief.\n".to_string())),
        settled: &[],
        diff: "diff --git a/a.rs b/a.rs\n+x\n",
    });
    assert!(first.starts_with(&shared_text) && second.starts_with(&shared_text));
    for part in ["Project facts.", "The brief.", "+x"] {
        assert!(shared_text.contains(part), "{part}");
    }
    for own in [
        "Group:",
        "question of",
        "Rule A body.",
        "Rule B body.",
        "How to answer",
    ] {
        assert!(!shared_text.contains(own), "{own} is in the shared part");
    }
    assert!(
        first.contains("Put your answer in `rules`")
            && second.contains("Put your answer in `summary` and `sites`")
    );
    assert!(second.contains("every site in the diff that handles rows a query returned"));
}

#[test]
fn every_group_gets_the_same_schema() {
    assert_eq!(schema(), schema());
    assert!(
        schema()["properties"]["sites"].is_object() && schema()["properties"]["rules"].is_object()
    );
}
