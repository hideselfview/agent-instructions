use super::*;

fn source(name: &str, text: &str) -> Source {
    Source {
        name: name.to_string(),
        text: text.to_string(),
    }
}

const RULE: &str = "Name the actor and the action in every sentence.\nUse the words for what happens, with no figure of speech.\nSay the verb. Never open with It or This.\nA fourth line that a brief does not paste.\n";

#[test]
fn a_brief_that_holds_three_lines_of_a_rule_in_a_row_pastes_it() {
    let brief = "Make the command print its input.\n\n  Name the actor and the action in every sentence.\n  Use the words for what happens, with no figure of speech.\n  Say the verb. Never open with It or This.\n";
    let found = pasted_from(brief, &[source("rules/text/plain.md", RULE)]).unwrap();
    assert_eq!(found.source, "rules/text/plain.md");
    assert_eq!(
        found.line,
        "Name the actor and the action in every sentence."
    );
}

#[test]
fn a_brief_that_quotes_one_or_two_lines_of_a_rule_pastes_nothing() {
    let brief = "Follow the rule: Name the actor and the action in every sentence.\n\nName the actor and the action in every sentence.\nUse the words for what happens, with no figure of speech.\n";
    assert_eq!(
        pasted_from(brief, &[source("rules/text/plain.md", RULE)]),
        None
    );
}

#[test]
fn the_lines_of_a_run_may_be_wrapped_at_other_indents_but_not_reordered() {
    let reordered = "Use the words for what happens, with no figure of speech.\nName the actor and the action in every sentence.\nSay the verb. Never open with It or This.\n";
    assert_eq!(
        pasted_from(reordered, &[source("rules/text/plain.md", RULE)]),
        None
    );
}

#[test]
fn short_runs_of_short_lines_are_no_paste() {
    let text = "a\nb\nc\nd\n";
    assert_eq!(pasted_from("a\nb\nc\n", &[source("s", text)]), None);
}

#[test]
fn the_workflow_page_and_every_rule_of_the_checkout_are_sources() {
    let scratch = crate::testing::Scratch::new("pasted-sources");
    let root = scratch.dir.join("instructions");
    std::fs::create_dir_all(root.join("rules/text")).unwrap();
    std::fs::create_dir_all(root.join("projects")).unwrap();
    std::fs::write(
        root.join("rules/text/plain.md"),
        format!("---\npaths: ['**/*']\n---\n{RULE}"),
    )
    .unwrap();
    let names: Vec<String> = sources(&root)
        .unwrap()
        .into_iter()
        .map(|source| source.name)
        .collect();
    assert_eq!(
        names,
        [
            "rules/text/plain.md",
            "builds help workflow",
            "builds help lists overview",
            "builds help workflow, coordinator",
            "builds help workflow, design",
            "builds help workflow, judge",
            "builds help lists overview, the order of the stack"
        ]
    );
}
