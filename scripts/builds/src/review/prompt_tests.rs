use super::*;
use crate::review::rules::Rule;

fn group(name: &str, enumerate: bool) -> Group {
    Group {
        name: name.to_string(),
        question: format!("question of {name}"),
        model: crate::review::Selection::Named("sonnet".to_string()),
        dir: String::new(),
        rules: vec!["r".to_string()],
        instructions: None,
        project_facts: false,
        enumerate,
        sites: enumerate.then(|| "handles rows a query returned".to_string()),
        commit_text: false,
        skips_comments: false,
        copy: false,
        passes: 1,
    }
}

fn parts_of(group: &Group, rule: &Rule, diff: &str) -> Parts {
    build(&Input {
        group,
        rules: &[rule],
        project_text: "Project facts.\n",
        brief: Some(&Brief {
            name: "plans/b.md".to_string(),
            text: "The brief.\n".to_string(),
        }),
        diff,
        context: "7: let x = 1;\n",
        open: &[],
        commits: "",
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
        "For new findings",
    ] {
        assert!(first.system.contains(part), "{part}");
    }
    assert!(!first.system.contains("+x") && !first.system.contains("let x = 1"));
    assert!(first.system.contains("open it before judging"));
    for prompt in [&first.system, &second.system] {
        assert!(prompt.contains("Use the old file's line number when you quote removed code."));
    }
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
        first.system.contains("For new findings, use `rules`")
            && second
                .system
                .contains("For new findings, use `summary` and `sites`")
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
fn the_setup_a_call_records_follows_the_rules_settings_and_passes_but_not_the_model() {
    let rule = Rule::parse("a", "---\npaths: ['**']\n---\nRule A body.\n");
    let mut one = group("one", true);
    one.passes = 2;
    one.commit_text = true;
    let reviewer = crate::review::Program::new("claude".into(), "/tmp".into());
    let recorded = setup(&one, &[&rule], &reviewer.select(&one.model));
    assert!(recorded.contains("Rule A body.") && recorded.contains("passes: 2"));
    assert!(
        !recorded.contains("Project facts.") && !recorded.contains("The brief."),
        "what each review brings is not part of the setup"
    );
    // The rules' text, the group's settings and its passes each change it;
    // the model is recorded beside it.
    let edited = Rule::parse("a", "---\npaths: ['**']\n---\nRule A body, edited.\n");
    assert_ne!(
        setup(&one, &[&edited], &reviewer.select(&one.model)),
        recorded
    );
    let mut asked_otherwise = one.clone();
    asked_otherwise.instructions = Some("Look harder.".to_string());
    assert_ne!(
        setup(
            &asked_otherwise,
            &[&rule],
            &reviewer.select(&asked_otherwise.model)
        ),
        recorded
    );
    let mut once = one.clone();
    once.passes = 1;
    assert_ne!(
        setup(&once, &[&rule], &reviewer.select(&once.model)),
        recorded
    );
    let mut on_haiku = one.clone();
    on_haiku.model = crate::review::Selection::Named("haiku".to_string());
    assert_eq!(
        setup(&on_haiku, &[&rule], &reviewer.select(&on_haiku.model)),
        recorded
    );
}

#[test]
fn the_diff_a_message_held_reads_back_out_of_it() {
    let rule = Rule::parse("r", "---\npaths: ['**']\n---\nBody.\n");
    let diff = "diff --git a/a.rs b/a.rs\n+let x = 1;\n";
    let parts = parts_of(&group("g", true), &rule, diff);
    assert_eq!(diff_of_message(&parts.message), Some(diff.trim_end()));
    assert_eq!(diff_of_message("no diff here"), None);
}
