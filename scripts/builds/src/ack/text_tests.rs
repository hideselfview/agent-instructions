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
    (0..count)
        .map(|n| format!("The reader checks line {n} and also considers it carefully."))
        .collect()
}

/// A random source that counts up from <from>, one draw at a time.
fn counting(from: usize) -> impl FnMut() -> Result<usize> {
    let mut next = from;
    move || {
        next += 1;
        Ok(next)
    }
}

#[test]
fn a_part_shows_each_made_up_word_at_its_place_and_says_how_many_it_holds() {
    let lines = vec![
        "===== rules/a.md (section 1 of 1) =====".to_string(),
        "A reader considers elephants carefully.".to_string(),
    ];
    // The places are "reader", "considers", "elephants" and "carefully".
    let made_up = [
        MadeUp {
            place: 0,
            word: "kodamir".to_string(),
        },
        MadeUp {
            place: 2,
            word: "vesolin".to_string(),
        },
    ];
    let shown = show_part(&lines, 2, 5, "abcd1234", &made_up).unwrap();
    let shown: Vec<&str> = shown.lines().collect();
    assert!(shown[0].contains("part 2 of 5 of version abcd1234"));
    assert!(shown[0].contains("builds put 2 made-up words into its sentences"));
    assert_eq!(shown[1], lines[0]);
    assert_eq!(
        shown[2],
        "A kodamir reader considers vesolin elephants carefully."
    );
    assert_eq!(shown[3], "builds ack: the end of part 2 of 5.");
}

#[test]
fn made_up_words_come_one_from_each_third_in_order_and_only_where_builds_put_them() {
    let lines = prose(30);
    let made_up = pick_made_up(&lines, &mut counting(0)).unwrap();
    assert_eq!(made_up.len(), WORDS_PER_PART);
    let shown = show_part(&lines, 1, 1, "v", &made_up).unwrap();
    let shown: Vec<&str> = shown.lines().skip(1).take(lines.len()).collect();
    for (third, made) in made_up.iter().enumerate() {
        assert!(
            !lines.iter().any(|line| line.contains(made.word.as_str())),
            "{made:?}"
        );
        let at: Vec<usize> = (0..shown.len())
            .filter(|n| shown[*n].contains(made.word.as_str()))
            .collect();
        assert_eq!(at.len(), 1, "{made:?}");
        assert!((third * 10..(third + 1) * 10).contains(&at[0]), "{made:?}");
        // Taking the made-up word out leaves the line the text holds.
        assert_eq!(
            shown[at[0]].replacen(&format!("{} ", made.word), "", 1),
            lines[at[0]]
        );
    }
    let places: Vec<usize> = made_up.iter().map(|made| made.place).collect();
    assert!(places.is_sorted(), "{places:?}");
}

#[test]
fn a_place_is_the_start_of_a_lowercase_word_of_running_prose_outside_code() {
    let lines = vec![
        "===== the heading names a section =====".to_string(),
        "Run `cargo check here` with Capitalized words, then stop.".to_string(),
        "A path some/dir/long-name and snake_case stay (as \"quoted\" words).".to_string(),
    ];
    let words: Vec<&str> = places(&lines)
        .into_iter()
        .map(|(line, at)| {
            let rest = &lines[line][at..];
            &rest[..rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap()]
        })
        .collect();
    assert_eq!(
        words,
        vec![
            "with", "words", "then", "stop", "path", "and", "stay", "as", "quoted", "words"
        ]
    );
}

#[test]
fn a_made_up_word_has_three_syllables_and_is_no_word_of_its_part() {
    // Nine draws of 0 make "bababa", the first syllable of each list three
    // times; nine draws of 1 make "dedede".
    let lines = vec!["The bababa text, and more words here.".to_string()];
    let mut draws = std::iter::once(0)
        .chain(std::iter::repeat_n(0, 9))
        .chain(std::iter::repeat_n(1, 9))
        .chain(1..);
    let made_up = pick_made_up(&lines, &mut || Ok(draws.next().unwrap())).unwrap();
    assert_eq!(made_up[0].word, "dedede");
    let words: std::collections::BTreeSet<&str> =
        made_up.iter().map(|made| made.word.as_str()).collect();
    assert_eq!(words.len(), WORDS_PER_PART);
}

#[test]
fn random_numbers_differ_on_each_draw() {
    assert_ne!(random().unwrap(), random().unwrap());
}

#[test]
fn a_place_the_part_lacks_is_refused() {
    let lines = vec!["Nothing here.".to_string()];
    let made_up = [MadeUp {
        place: 1,
        word: "kodamir".to_string(),
    }];
    assert!(show_part(&lines, 1, 1, "v", &made_up).is_err());
}

#[test]
fn a_part_with_too_few_places_is_refused() {
    let lines = vec!["Only two words.".to_string()];
    assert!(pick_made_up(&lines, &mut counting(0)).is_err());
}
