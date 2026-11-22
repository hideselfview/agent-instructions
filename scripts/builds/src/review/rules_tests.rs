use super::*;

#[test]
fn loading_requires_a_shared_rules_directory() {
    let scratch = crate::testing::Scratch::new("missing-shared-rules");
    let error = load_shared(&scratch.dir).unwrap_err().text();
    assert!(error.contains("listing") && error.contains("/rules"));
}
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
fn loading_project_rules_requires_the_projects_directory() {
    let scratch = crate::testing::Scratch::new("missing-project-rules");
    std::fs::create_dir(scratch.dir.join("rules")).unwrap();
    let error = load_project(&scratch.dir, "bae").unwrap_err().text();
    assert!(error.contains("listing") && error.contains("/projects"));
}

#[test]
fn an_explicit_rule_directory_must_exist() {
    let scratch = crate::testing::Scratch::new("unknown-brief-project");
    std::fs::create_dir(scratch.dir.join("rules")).unwrap();
    std::fs::create_dir(scratch.dir.join("projects")).unwrap();
    let error = load_project(&scratch.dir, "missing").unwrap_err().text();
    assert!(error.contains("listing") && error.contains("/projects/missing-rules"));
}

#[test]
fn a_rule_with_no_paths_covers_every_path_and_inline_lists_parse() {
    let none = Rule::parse("r", "---\ndigest: d\nblocking: false\n---\nbody\n");
    assert!(none.paths.is_none() && none.matches(&["x".to_string()], &Globs::default()));
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

#[test]
fn a_rule_covers_a_design_file_only_when_its_front_matter_says_it_judges_design_files() {
    let design = Globs {
        places: vec!["design/*.html".into()],
        ..Globs::default()
    };
    let code = Rule::parse("c", "---\npaths:\n  - '**/*'\n---\nbody\n");
    let looks = Rule::parse("l", "---\npaths:\n  - '**/*'\ndesign: true\n---\nbody\n");
    assert!(!code.design && looks.design);
    let board = ["design/a.html".to_string()];
    assert!(!code.matches(&board, &design));
    assert!(looks.matches(&board, &design));
    let both = ["design/a.html".to_string(), "a.rs".to_string()];
    assert!(code.matches(&both, &design) && looks.matches(&both, &design));
    // A repo with no design globs has no design files.
    assert!(code.matches(&board, &Globs::default()));
}

#[test]
fn a_rule_covers_a_capture_file_only_when_its_front_matter_says_it_judges_capture_files() {
    let files = Globs {
        places: vec!["design/*.html".into()],
        capture: vec!["app/shots".into()],
        ..Globs::default()
    };
    let code = Rule::parse("c", "---\npaths:\n  - '**/*'\n---\nbody\n");
    let looks = Rule::parse("l", "---\npaths:\n  - '**/*'\ndesign: true\n---\nbody\n");
    let shots = Rule::parse("s", "---\npaths:\n  - '**/*'\ncapture: true\n---\nbody\n");
    assert!(!code.capture && !looks.capture && shots.capture && !shots.design);
    let shot = ["app/shots/Components/Row.swift".to_string()];
    assert!(!code.matches(&shot, &files) && !looks.matches(&shot, &files));
    assert!(shots.matches(&shot, &files));
    // A capture rule judges no board.
    let board = ["design/a.html".to_string()];
    assert!(!shots.matches(&board, &files) && looks.matches(&board, &files));
    // A repo with no capture folders has no capture files.
    assert!(code.matches(&shot, &Globs::default()));
}
