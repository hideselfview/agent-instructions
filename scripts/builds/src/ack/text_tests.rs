use super::*;

fn names(text: &Text) -> Vec<String> {
    text.sections.iter().map(|s| s.name.clone()).collect()
}

const COORDINATOR_HEADINGS: [&str; 4] = [
    "PLAN, BRIEF, EXECUTE",
    "WHAT A PLANNER DOES",
    "DISPATCH",
    "HEALTH CHECK",
];
const DESIGN_HEADINGS: [&str; 2] = ["DESIGN CHANGES", "PUBLISHING DESIGNS"];

fn headings_of(text: &Text, section: &str) -> Vec<String> {
    let section = text.sections.iter().find(|s| s.name == section).unwrap();
    crate::help::sections(&section.text)
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_text_every_agent_reads_holds_no_section_of_a_role() {
    let text = Text::for_roles(&[]);
    assert_eq!(
        names(&text),
        vec!["builds help workflow", "builds help lists overview"]
    );
    let workflow = headings_of(&text, "builds help workflow");
    for heading in ["ROLES", "WORKTREES", "REVIEW", "LANDING", "DESIGN TERMS"] {
        assert!(workflow.iter().any(|held| held == heading), "{heading}");
    }
    for heading in COORDINATOR_HEADINGS
        .iter()
        .chain(&DESIGN_HEADINGS)
        .chain(&["JUDGING"])
    {
        assert!(!workflow.iter().any(|held| held == heading), "{heading}");
    }
    assert_eq!(
        headings_of(&text, "builds help lists overview"),
        ["WHO WRITES WHAT"]
    );
    assert!(text.sections[0].text.starts_with("builds workflow: "));
}

#[test]
fn each_role_adds_the_sections_of_its_role() {
    let coordinator = Text::for_roles(&[Role::Coordinator]);
    assert_eq!(
        names(&coordinator),
        vec![
            "builds help workflow",
            "builds help lists overview",
            "builds help workflow, coordinator",
            "builds help lists overview, the order of the stack",
        ]
    );
    assert_eq!(
        headings_of(&coordinator, "builds help workflow, coordinator"),
        COORDINATOR_HEADINGS
    );
    let design = Text::for_roles(&[Role::Design]);
    assert_eq!(
        headings_of(&design, "builds help workflow, design"),
        DESIGN_HEADINGS
    );
    let judge = Text::for_roles(&[Role::Judge]);
    assert_eq!(
        headings_of(&judge, "builds help workflow, judge"),
        ["JUDGING"]
    );
    assert_eq!(
        headings_of(&judge, "builds help lists overview, the order of the stack"),
        ["EFFECTS AND ORDER OF THE STACK"]
    );
    let two = Text::for_roles(&[Role::Judge, Role::Design]);
    assert_eq!(two.sections.len(), 5, "{:?}", names(&two));
}

#[test]
fn every_section_of_every_page_is_read_by_some_role_and_none_is_lost() {
    let everything = Text::for_roles(&Role::ALL);
    let mut held: Vec<String> = Vec::new();
    for section in &everything.sections {
        held.extend(
            crate::help::sections(&section.text)
                .into_iter()
                .map(str::to_string),
        );
    }
    let mut want: Vec<String> = crate::help::sections(crate::help::WORKFLOW_HELP)
        .into_iter()
        .chain(crate::help::sections(crate::help::LISTS_OVERVIEW))
        .map(str::to_string)
        .collect();
    held.sort();
    want.sort();
    assert_eq!(held, want);
    let lines = |text: &str| text.lines().filter(|l| !l.is_empty()).count();
    let held_lines: usize = everything.sections.iter().map(|s| lines(&s.text)).sum();
    assert_eq!(
        held_lines,
        lines(crate::help::WORKFLOW_HELP) + lines(crate::help::LISTS_OVERVIEW)
    );
}

#[test]
fn an_agent_holds_the_roles_whose_sections_it_acknowledged() {
    let text = Text::for_roles(&[Role::Judge, Role::Coordinator]);
    assert_eq!(
        Text::roles_held(&text.hashes()),
        vec![Role::Coordinator, Role::Judge]
    );
    assert!(Text::roles_held(&Text::for_roles(&[]).hashes()).is_empty());
}

#[test]
fn a_role_is_named_by_its_word_and_no_other() {
    assert_eq!(Role::parse("design").unwrap(), Role::Design);
    assert!(Role::parse("planner").is_err());
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
    let text = Text::for_roles(&[]);
    let mut known = text.hashes();
    assert!(text.unread(&known).sections.is_empty());
    known.remove(0);
    known[0].1 = "changed".to_string();
    assert_eq!(
        names(&text.unread(&known)),
        vec!["builds help workflow", "builds help lists overview"]
    );
}
