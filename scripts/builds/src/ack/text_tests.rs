use super::*;
use crate::testing::{Scratch, repo};

fn rule(paths: &str, body: &str) -> String {
    format!("---\ndigest: d\npaths:\n  - '{paths}'\nblocking: true\n---\n\n{body}\n")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A home whose agent-instructions checkout holds a rule for every file, one
/// for Rust files only, and a bae rule for every file, with a
/// ~/.claude/CLAUDE.md.
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
fn the_text_holds_the_helps_the_rules_for_every_file_and_both_claude_files() {
    let scratch = Scratch::new("ack-text");
    let places = home(&scratch);
    let checkout = scratch.dir.join("bae");
    repo(&checkout, "target 1\n");
    write(&checkout.join("CLAUDE.md"), "bae's rules.\n");
    let text = Text::read(&places, "bae", Checkout::at(&checkout).unwrap().as_ref()).unwrap();
    assert_eq!(
        names(&text),
        vec![
            "builds help",
            "builds help workflow",
            "builds help lists",
            "builds help briefs",
            "rules/text/plain.md",
            "projects/bae-rules/screen/no-notes.md",
            "~/.claude/CLAUDE.md",
            "the CLAUDE.md of repo bae",
        ]
    );
    let builds = Text::read(&places, "builds", None).unwrap();
    assert!(!names(&builds).contains(&"projects/bae-rules/screen/no-notes.md".to_string()));
}

#[test]
fn a_repo_whose_claude_file_is_the_global_one_adds_no_section() {
    let scratch = Scratch::new("ack-text-same");
    let places = home(&scratch);
    let checkout = scratch.dir.join("agent-instructions-checkout");
    repo(&checkout, "target 1\n");
    write(&checkout.join("CLAUDE.md"), "Global rules.\n");
    let text = Text::read(&places, "builds", Checkout::at(&checkout).unwrap().as_ref()).unwrap();
    assert_eq!(names(&text).last().unwrap(), "~/.claude/CLAUDE.md");
}

#[test]
fn an_edit_of_a_rule_changes_the_version_and_names_that_rule() {
    let scratch = Scratch::new("ack-text-edit");
    let places = home(&scratch);
    let before = Text::read(&places, "bae", None).unwrap();
    write(
        &scratch.dir.join("agent-instructions/rules/text/plain.md"),
        &rule("**/*", "Write plain, literal sentences."),
    );
    let after = Text::read(&places, "bae", None).unwrap();
    assert_ne!(before.version(), after.version());
    assert_eq!(
        after.changed_since(&before.hashes()),
        vec!["rules/text/plain.md"]
    );
    assert!(after.changed_since(&after.hashes()).is_empty());
}

fn long_text() -> Text {
    let section = |name: &str, lines: usize| Section {
        name: name.to_string(),
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
    let parts = text.parts(&[]);
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
fn the_changed_sections_come_first() {
    let parts = long_text().parts(&["second".to_string()]);
    assert_eq!(parts[0][0], "===== second (section 1 of 2) =====");
}

#[test]
fn a_part_shows_its_words_at_its_start_middle_and_end() {
    let lines: Vec<String> = (0..10).map(|n| format!("line {n}")).collect();
    let words = vec!["kama".to_string(), "tobi".to_string(), "rusa".to_string()];
    let shown = show_part(&lines, 2, 5, "abcd1234", &words);
    let shown: Vec<&str> = shown.lines().collect();
    assert_eq!(shown[1], "CHECK WORD 1 OF 3 IN PART 2: kama");
    assert_eq!(shown[7], "CHECK WORD 2 OF 3 IN PART 2: tobi");
    assert_eq!(shown[shown.len() - 2], "CHECK WORD 3 OF 3 IN PART 2: rusa");
    assert!(shown[0].contains("part 2 of 5 of version abcd1234"));
}

#[test]
fn new_words_differ_on_each_draw_and_hold_only_lowercase_letters() {
    let first = new_words(6).unwrap();
    assert_eq!(first.len(), 6);
    assert!(
        first
            .iter()
            .all(|word| word.len() == 6 && word.chars().all(|c| c.is_ascii_lowercase()))
    );
    assert_ne!(first, new_words(6).unwrap());
}
