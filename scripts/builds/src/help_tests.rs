use super::*;

#[test]
fn each_page_lists_its_section_headings() {
    assert!(sections(WORKFLOW_HELP).contains(&"DISPATCH"));
    assert!(sections(WORKFLOW_HELP).contains(&"PLAN, BRIEF, EXECUTE"));
    assert!(sections(HELP).contains(&"LANDING AND REVIEW"));
    assert!(sections(LISTS_HELP).contains(&"RULINGS"));
    assert!(sections(BRIEFS_HELP).contains(&"READ FIRST"));
    assert_eq!(page(Some("nothing")), None);
}
