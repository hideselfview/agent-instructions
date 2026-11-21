use super::*;
use crate::testing::Scratch;
use std::path::Path;

fn rule(paths: &str, body: &str) -> String {
    format!("---\ndigest: d\npaths:\n  - '{paths}'\nblocking: true\n---\n\n{body}\n")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A home whose agent-instructions checkout holds a rule for every file, one
/// for Rust files only, and a bae rule for every file, with a
/// ~/.claude/CLAUDE.md, which the text leaves out.
fn home(scratch: &Scratch) -> Places {
    let places = scratch.places();
    let root = scratch.dir.join("agent-instructions");
    write(
        &root.join("rules/text/plain.md"),
        &rule("**/*", "Write plain sentences."),
    );
    write(
        &root.join("rules/code/rust-only.md"),
        &rule("**/*.rs", "Rust only."),
    );
    write(
        &root.join("projects/bae-rules/screen/no-notes.md"),
        &rule("**/*", "No music notes."),
    );
    std::fs::create_dir_all(places.home.join(".codex")).unwrap();
    std::os::unix::fs::symlink(&root, places.home.join(".codex/agent-instructions")).unwrap();
    write(&places.home.join(".claude/CLAUDE.md"), "Global rules.\n");
    places
}

fn names(text: &Text) -> Vec<String> {
    text.sections.iter().map(|s| s.name.clone()).collect()
}

#[test]
fn the_text_holds_the_workflow_the_lists_overview_and_the_rules_for_every_file_and_no_claude_md() {
    let scratch = Scratch::new("ack-text");
    let places = home(&scratch);
    let text = Text::read(&places, "bae").unwrap();
    assert_eq!(
        names(&text),
        vec![
            "builds help workflow",
            "builds help lists overview",
            "rules/text/plain.md",
            "projects/bae-rules/screen/no-notes.md",
        ]
    );
    let builds = Text::read(&places, "builds").unwrap();
    assert!(!names(&builds).contains(&"projects/bae-rules/screen/no-notes.md".to_string()));
}

#[test]
fn an_edit_of_a_rule_changes_the_version_and_names_that_rule() {
    let scratch = Scratch::new("ack-text-edit");
    let places = home(&scratch);
    let before = Text::read(&places, "bae").unwrap();
    write(
        &scratch.dir.join("agent-instructions/rules/text/plain.md"),
        &rule("**/*", "Write plain, literal sentences."),
    );
    let after = Text::read(&places, "bae").unwrap();
    assert_ne!(before.version(), after.version());
    assert_eq!(
        after.changed_since(&before.hashes()),
        vec!["rules/text/plain.md"]
    );
    assert!(after.changed_since(&after.hashes()).is_empty());
    assert!(after.removed_since(&after.hashes()).is_empty());
}

#[test]
fn a_section_the_text_lost_is_removed_and_not_changed() {
    let text = long_text();
    let mut acknowledged = text.hashes();
    acknowledged.push(("gone".to_string(), "00".to_string()));
    assert_eq!(text.removed_since(&acknowledged), vec!["gone"]);
    assert!(text.changed_since(&acknowledged).is_empty());
}

fn long_text() -> Text {
    let section = |name: &str, lines: usize| Section {
        name: name.to_string(),
        how_to_read: name.to_string(),
        text: (0..lines)
            .map(|n| format!("{name} line {n} {}", "x".repeat(60)))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    Text {
        sections: vec![section("first", 600), section("second", 50)],
    }
}

#[test]
fn each_part_stays_under_the_limit_and_the_parts_hold_every_line() {
    let text = long_text();
    let parts = text.parts();
    assert!(parts.len() > 1);
    for part in &parts {
        let bytes: usize = part.iter().map(|line| line.len() + 1).sum();
        assert!(bytes <= PART_BYTES, "{bytes}");
    }
    let all: Vec<&String> = parts.iter().flatten().collect();
    for section in &text.sections {
        for line in section.text.lines() {
            assert!(all.iter().any(|held| *held == line), "{line}");
        }
    }
    assert!(parts[1][0].starts_with("===== first, continued"));
}

#[test]
fn random_numbers_differ_on_each_draw() {
    assert_ne!(random().unwrap(), random().unwrap());
}

#[test]
fn a_part_shows_its_lines_unchanged_between_its_two_marker_lines_and_ends_with_the_next_command() {
    let lines = vec![
        "===== rules/a.md (section 1 of 1) =====".to_string(),
        "A reader considers elephants carefully.".to_string(),
    ];
    let shown = show_part(&lines, 2, 3, "v1", "  builds ack part 3 tok");
    let shown: Vec<&str> = shown.lines().collect();
    assert_eq!(shown[0], "builds ack: part 2 of 3 of version v1.");
    assert_eq!(&shown[1..3], &lines[..]);
    assert_eq!(shown[3], "builds ack: the end of part 2 of 3.");
    assert_eq!(shown[4], "  builds ack part 3 tok");
    assert_eq!(shown.len(), 5);
}
