use super::*;
use crate::command::Command;

#[test]
fn each_page_lists_its_section_headings() {
    assert!(sections(WORKFLOW_HELP).contains(&"DISPATCH"));
    assert!(sections(WORKFLOW_HELP).contains(&"PLAN, BRIEF, EXECUTE"));
    assert!(sections(&HELP).contains(&"LANDING AND REVIEW"));
    assert!(sections(&LISTS_HELP).contains(&"DECISION LOG"));
    assert!(sections(BRIEFS_HELP).contains(&"READ FIRST"));
    assert_eq!(page(Some("nothing")), None);
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

#[test]
fn lookup_prints_a_page_or_one_of_its_sections() {
    assert_eq!(lookup(&[]), Ok(HELP.as_str()));
    assert_eq!(lookup(&words(&["workflow"])), Ok(WORKFLOW_HELP));
    let dispatch = lookup(&words(&["workflow", "DISPATCH"])).unwrap();
    assert!(dispatch.starts_with("DISPATCH\n"));
    assert!(!dispatch.contains("\nREVIEW\n"));
    assert!(WORKFLOW_HELP.contains(dispatch));
    let plan = lookup(&words(&["workflow", "plan,", "brief,", "execute"])).unwrap();
    assert!(plan.starts_with("PLAN, BRIEF, EXECUTE\n"));
    assert!(
        lookup(&words(&["APP", "BUILDS"]))
            .unwrap()
            .starts_with("APP BUILDS\n")
    );
    assert!(
        lookup(&words(&["lists", "DECISION LOG"]))
            .unwrap()
            .starts_with("DECISION LOG\n")
    );
}

#[test]
fn lookup_names_the_headings_when_no_section_matches() {
    let why = lookup(&words(&["workflow", "NOTHING"])).unwrap_err();
    assert!(
        why.starts_with("builds help workflow has no section NOTHING. Its sections are ROLES;")
    );
    assert!(
        lookup(&words(&["nothing"]))
            .unwrap_err()
            .starts_with("builds help has no section nothing.")
    );
}

/// Help shows each usage line a command declares once, through a marker, and
/// no usage line of its own: every line of its text that starts as a usage
/// line does is a marker's.
#[test]
fn help_shows_each_declared_usage_line_once_and_no_other() {
    let mut shown: Vec<&str> = Vec::new();
    for page in [HELP_SOURCE, LISTS_SOURCE, WORKFLOW_HELP, BRIEFS_HELP] {
        for line in page.lines() {
            assert!(
                !line.starts_with("  builds "),
                "help writes a usage line of its own: {line}"
            );
            if let Some(marker) = marker(line) {
                shown.extend(marker.lines);
            } else {
                assert!(
                    !line.starts_with("  {"),
                    "a marker names no command: {line}"
                );
            }
        }
    }
    for command in Command::ALL {
        for usage in command.usage() {
            let times = shown.iter().filter(|shown| *shown == usage).count();
            assert_eq!(times, 1, "help shows {usage:?} {times} times");
        }
    }
}

/// A usage line wider than help's width breaks between its arguments, and
/// each further line starts under the first argument.
#[test]
fn a_long_usage_line_breaks_between_its_arguments() {
    let item = Command::Item(crate::command::item::Edit);
    assert_eq!(
        wrapped(item, item.usage()[0]),
        "  builds item edit N [--title T] [--area A] [--details-file F] [--brief P]\n                   [--for F] [--origin N] [--tag T]... [--untag T]..."
    );
}

#[test]
fn a_marker_puts_its_description_beside_the_usage_line_when_it_fits() {
    assert_eq!(
        with_usage("  {version}                   the hash\nprose\n"),
        "  builds version              the hash\nprose\n"
    );
    assert_eq!(
        with_usage("  {du}  the KiB\n"),
        "  builds du <path>...\n        the KiB\n"
    );
    assert_eq!(
        with_usage("  {status}\n"),
        "  builds status\n  builds status --json [--no-folders]\n"
    );
    assert_eq!(
        with_usage("  {status #2}"),
        "  builds status --json [--no-folders]"
    );
    assert_eq!(with_usage("  {nothing}\n"), "  {nothing}\n");
}
