use super::*;

#[test]
fn each_page_lists_its_section_headings() {
    assert!(sections(WORKFLOW_HELP).contains(&"DISPATCH"));
    assert!(sections(WORKFLOW_HELP).contains(&"PLAN, BRIEF, EXECUTE"));
    assert!(sections(HELP).contains(&"LANDING AND REVIEW"));
    assert!(sections(LISTS_HELP).contains(&"DECISION LOG"));
    assert!(sections(BRIEFS_HELP).contains(&"READ FIRST"));
    assert_eq!(page(Some("nothing")), None);
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

#[test]
fn lookup_prints_a_page_or_one_of_its_sections() {
    assert_eq!(lookup(&[]), Ok(HELP));
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
