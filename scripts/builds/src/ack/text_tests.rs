use super::*;

fn names(text: &Text) -> Vec<String> {
    text.sections.iter().map(|s| s.name.clone()).collect()
}

#[test]
fn the_text_holds_only_the_workflow_and_the_lists_overview() {
    let text = Text::read();
    assert_eq!(
        names(&text),
        vec!["builds help workflow", "builds help lists overview"]
    );
    assert_eq!(text.sections[0].text, crate::help::WORKFLOW_HELP);
    assert_eq!(text.sections[1].text, crate::help::LISTS_OVERVIEW);
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
        "===== Example instructions (section 1 of 1) =====".to_string(),
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

#[test]
fn the_unread_sections_are_those_whose_name_and_hash_the_agent_lacks() {
    let text = Text::read();
    let mut known = text.hashes();
    assert!(text.unread(&known).sections.is_empty());
    known.remove(0);
    known[0].1 = "changed".to_string();
    assert_eq!(
        names(&text.unread(&known)),
        vec!["builds help workflow", "builds help lists overview"]
    );
}
