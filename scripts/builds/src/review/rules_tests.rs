use super::*;

#[test]
fn front_matter_gives_paths_blocking_and_review() {
    let rule = Rule::parse(
        "r",
        "---\ndigest: d\npaths:\n  - '**/*.rs'\n  - bae-web/**/*.rs\nblocking: true\nreview: false\n---\n\nbody\n",
    );
    assert_eq!(
        rule.paths,
        Some(vec!["**/*.rs".to_string(), "bae-web/**/*.rs".to_string()])
    );
    assert!(rule.blocking && !rule.reviewable);
}

#[test]
fn a_rule_with_no_paths_covers_every_path_and_inline_lists_parse() {
    let none = Rule::parse("r", "---\ndigest: d\nblocking: false\n---\nbody\n");
    assert!(none.paths.is_none() && none.matches(&["x".to_string()]));
    assert!(!none.blocking && none.reviewable);
    let inline = Rule::parse("r", "---\npaths: ['a/*.rs', \"b\"]\n---\nx\n");
    assert_eq!(
        inline.paths,
        Some(vec!["a/*.rs".to_string(), "b".to_string()])
    );
}

#[test]
fn globs_follow_gitignore_style() {
    assert!(glob_matches("**/*.rs", "a.rs"));
    assert!(glob_matches("**/*.rs", "a/b/c.rs"));
    assert!(!glob_matches("**/*.rs", "a/b/c.swift"));
    assert!(glob_matches(
        "bae-bridge/**/*.rs",
        "bae-bridge/src/types/a.rs"
    ));
    assert!(!glob_matches("bae-bridge/**/*.rs", "bae-bridgex/a.rs"));
    assert!(glob_matches("**/*", "any/path.txt"));
    assert!(glob_matches("visible-ios/**", "visible-ios/App/a.swift"));
    assert!(glob_matches("bae-store/src/**/*.rs", "bae-store/src/a.rs"));
    assert!(!glob_matches("src/*.rs", "src/a/b.rs"));
    assert!(glob_matches("**/build.gradle*", "app/build.gradle.kts"));
    assert!(glob_matches("**/hooks/**", "a/hooks/b/pre-commit"));
    assert!(glob_matches("src/a?.rs", "src/ab.rs"));
    assert!(!glob_matches("src?a.rs", "src/a.rs"));
}
