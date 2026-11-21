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
fn the_text_holds_both_claude_files_the_workflow_the_lists_overview_and_the_rules_for_every_file() {
    let scratch = Scratch::new("ack-text");
    let places = home(&scratch);
    let checkout = scratch.dir.join("bae");
    repo(&checkout, "target 1\n");
    write(&checkout.join("CLAUDE.md"), "bae's rules.\n");
    let text = Text::read(&places, "bae", Checkout::at(&checkout).unwrap().as_ref()).unwrap();
    assert_eq!(
        names(&text),
        vec![
            "~/.claude/CLAUDE.md",
            "the CLAUDE.md of repo bae",
            "builds help workflow",
            "builds help lists overview",
            "rules/text/plain.md",
            "projects/bae-rules/screen/no-notes.md",
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
    assert_eq!(names(&text)[1], "builds help workflow");
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
        repo: None,
        sections: vec![section("first", 600), section("second", 50)],
    }
}

#[test]
fn each_part_stays_under_the_limit_and_the_parts_hold_every_line() {
    let text = long_text();
    let parts = text.parts(&["first".to_string(), "second".to_string()]);
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
fn the_parts_hold_only_the_sections_the_caller_names() {
    let parts = long_text().parts(&["second".to_string()]);
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0][0], "===== second (section 1 of 1) =====");
}

fn prose(count: usize) -> Vec<String> {
    let spelled = |n: usize| -> String {
        n.to_string()
            .bytes()
            .map(|digit| (b'a' + digit - b'0') as char)
            .collect()
    };
    (0..count)
        .map(|n| {
            format!(
                "The reader checks tokens{} and also considers elephants{} carefully.",
                spelled(n),
                spelled(n)
            )
        })
        .collect()
}

#[test]
fn a_part_shows_each_blanked_word_as_its_first_letter_and_underscores() {
    let lines = vec![
        "Nothing here is blanked.".to_string(),
        "A reader considers elephants carefully.".to_string(),
    ];
    let shown = show_part(&lines, 2, 5, "abcd1234", &["elephants".to_string()]).unwrap();
    let shown: Vec<&str> = shown.lines().collect();
    assert!(shown[0].contains("part 2 of 5 of version abcd1234"));
    assert!(shown[0].contains("1 words in it are blanked"));
    assert_eq!(shown[1], lines[0]);
    assert_eq!(shown[2], "A reader considers e________ carefully.");
    assert_eq!(shown[3], "builds ack: the end of part 2 of 5.");
}

#[test]
fn blanks_come_one_from_each_third_in_order_and_appear_nowhere_in_the_output() {
    let lines = prose(30);
    let mut next = 0;
    let mut random = || {
        next += 1;
        next
    };
    let words = pick_blanks(&lines, &mut random).unwrap();
    assert_eq!(words.len(), WORDS_PER_PART);
    let at = |word: &String| {
        lines
            .iter()
            .position(|line| line.contains(word.as_str()))
            .unwrap()
    };
    for (third, word) in words.iter().enumerate() {
        assert!((third * 10..(third + 1) * 10).contains(&at(word)), "{word}");
    }
    let shown = show_part(&lines, 1, 1, "v", &words).unwrap();
    for word in &words {
        assert!(!shown.contains(word.as_str()), "{word}");
    }
}

#[test]
fn a_blank_is_a_lowercase_word_of_running_prose_that_occurs_once_in_the_part() {
    let lines = vec![
        "Run `scripts/cargo.sh` with Capitalized words, repeated repeated.".to_string(),
        "A path some/dir/long-name and snake_case_names stay.".to_string(),
    ];
    let forbidden = [
        "scripts",
        "cargo",
        "capitalized",
        "repeated",
        "dir",
        "long",
        "name",
        "snake",
        "case",
        "names",
    ];
    for start in 0..6 {
        let mut draws = start..;
        for word in pick_blanks(&lines, &mut || draws.next().unwrap()).unwrap() {
            assert!(!forbidden.contains(&word.as_str()), "{word}");
        }
    }
}

#[test]
fn a_part_with_only_short_words_blanks_the_longest_it_has() {
    let lines = vec![
        "Add the big red cat.".to_string(),
        "Now all of us run.".to_string(),
    ];
    let words = pick_blanks(&lines, &mut || 0).unwrap();
    assert_eq!(words.len(), WORDS_PER_PART);
    assert!(words.iter().all(|word| word.len() == 3));
}

#[test]
fn random_numbers_differ_on_each_draw() {
    assert_ne!(random().unwrap(), random().unwrap());
}

#[test]
fn a_word_the_part_lacks_is_refused() {
    let lines = vec!["Nothing here.".to_string()];
    assert!(show_part(&lines, 1, 1, "v", &["elephants".to_string()]).is_err());
}

#[test]
fn a_part_with_too_few_blankable_words_is_refused() {
    let lines = vec!["Only two words.".to_string()];
    assert!(pick_blanks(&lines, &mut || 0).is_err());
}
