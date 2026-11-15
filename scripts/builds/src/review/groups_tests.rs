use super::*;
use crate::review::rules;

/// The checkout this file is in: builds sits in scripts/builds.
fn checkout() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the checkout resolves")
}

fn rule(slug: &str, paths: &str) -> Rule {
    Rule::parse(slug, &format!("---\npaths:\n  - '{paths}'\n---\nbody\n"))
}

#[test]
fn every_rule_of_the_checkout_is_in_exactly_one_group_and_every_slug_names_a_rule() {
    let root = checkout();
    let groups = Groups::load(&root).unwrap();
    let rules = rules::load(&root, None).unwrap();
    assert!(!rules.is_empty());
    assert_eq!(groups.faults(&rules), Vec::<String>::new());
}

#[test]
fn faults_name_a_rule_in_no_group_in_two_groups_or_missing() {
    let groups = Groups::parse(
        r#"
[[group]]
name = "a"
question = "q"
model = "sonnet"
rules = ["one", "two", "ghost"]

[[group]]
name = "b"
question = "q"
model = "gpt"
rules = ["two"]
"#,
    )
    .unwrap();
    let rules = vec![rule("one", "**"), rule("two", "**"), rule("three", "**")];
    let faults = groups.faults(&rules);
    assert!(
        faults.iter().any(|f| f.contains("ghost, which is no rule")),
        "{faults:?}"
    );
    assert!(
        faults
            .iter()
            .any(|f| f.contains("two is in group a and group b"))
    );
    assert!(faults.iter().any(|f| f.contains("three is in no group")));
    assert!(faults.iter().any(|f| f.contains("names model gpt")));
}

#[test]
fn a_group_runs_when_one_of_its_rules_covers_a_changed_path() {
    let groups = Groups::parse(
        r#"
[[group]]
name = "swift"
question = "q"
model = "sonnet"
rules = ["s1", "s2", "not-in-this-project"]

[[group]]
name = "rust"
question = "q"
model = "opus"
rules = ["r1"]
"#,
    )
    .unwrap();
    let rules = vec![
        rule("s1", "**/*.swift"),
        rule("s2", "**/*.kt"),
        rule("r1", "**/*.rs"),
    ];
    let picked = groups.pick(&rules, &["bae-macos/A.swift".to_string()]);
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].0.name, "swift");
    let slugs: Vec<&str> = picked[0].1.iter().map(|r| r.slug.as_str()).collect();
    assert_eq!(slugs, ["s1", "s2"]);
    assert!(groups.pick(&rules, &["README.md".to_string()]).is_empty());
}

#[test]
fn a_rule_marked_review_false_never_runs_its_group() {
    let groups = Groups::parse(
        "[[group]]\nname = \"g\"\nquestion = \"q\"\nmodel = \"sonnet\"\nrules = [\"x\"]\n",
    )
    .unwrap();
    let rules = vec![Rule::parse(
        "x",
        "---\npaths: ['**']\nreview: false\n---\nb\n",
    )];
    assert!(groups.pick(&rules, &["a".to_string()]).is_empty());
}

#[test]
fn a_group_that_lists_sites_says_what_a_site_is() {
    let groups = Groups::parse(
        "[[group]]\nname = \"g\"\nquestion = \"q\"\nmodel = \"opus\"\nenumerate = true\nrules = [\"a\", \"b\"]\n",
    )
    .unwrap();
    let rules = vec![rule("a", "**"), rule("b", "**")];
    assert!(
        groups
            .faults(&rules)
            .iter()
            .any(|f| f.contains("lists sites, so it says what a site is"))
    );
}
