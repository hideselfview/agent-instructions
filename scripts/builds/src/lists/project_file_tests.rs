use super::*;

const FILE: &str = "# demo\n\nDemo is a tool.\nIt runs here.\n\n## Crates\n\nSome text.\n\n## Hard limits\n\n- Never do a.\n- Never do b.\n\n### Detail\n\nMore.\n\n## After\n\nLater.\n";

fn root_with(name: &str, text: &str) -> (crate::testing::Scratch, std::path::PathBuf) {
    let scratch = crate::testing::Scratch::new("project-file");
    let root = scratch.dir.join("instructions");
    std::fs::create_dir_all(root.join("projects")).unwrap();
    std::fs::write(root.join("projects").join(format!("{name}.md")), text).unwrap();
    (scratch, root)
}

#[test]
fn the_description_is_the_first_paragraph_and_the_limits_are_their_section() {
    let (_scratch, root) = root_with("demo", FILE);
    let file = load(&root, "demo").unwrap();
    assert_eq!(file.description, "Demo is a tool.\nIt runs here.");
    assert_eq!(
        file.hard_limits,
        "- Never do a.\n- Never do b.\n\n### Detail\n\nMore."
    );
    assert!(file.path.ends_with("projects/demo.md"));
}

#[test]
fn a_missing_file_fails_naming_it() {
    let (_scratch, root) = root_with("demo", FILE);
    let error = load(&root, "other").unwrap_err().text();
    assert!(error.contains("projects/other.md"), "{error}");
}

#[test]
fn a_file_without_a_hard_limits_section_fails_naming_the_file() {
    let (_scratch, root) = root_with("demo", "# demo\n\nDemo is a tool.\n\n## Crates\n\nText.\n");
    let error = load(&root, "demo").unwrap_err().text();
    assert!(error.contains("projects/demo.md"), "{error}");
    assert!(error.contains("## Hard limits"), "{error}");
}

#[test]
fn an_empty_hard_limits_section_fails() {
    let (_scratch, root) = root_with(
        "demo",
        "# demo\n\nDemo is a tool.\n\n## Hard limits\n\n## Next\n\nx\n",
    );
    assert!(load(&root, "demo").is_err());
}

#[test]
fn a_file_with_a_heading_where_the_description_belongs_fails() {
    let (_scratch, root) = root_with("demo", "# demo\n\n## Hard limits\n\n- a\n");
    let error = load(&root, "demo").unwrap_err().text();
    assert!(error.contains("no description"), "{error}");
}

#[test]
fn the_pasted_text_check_reads_the_limits_of_a_project_with_a_file_and_none_for_a_missing_one() {
    let (_scratch, root) = root_with("demo", FILE);
    assert_eq!(
        hard_limits_if_any(&root, "demo").unwrap().unwrap(),
        "- Never do a.\n- Never do b.\n\n### Detail\n\nMore."
    );
    assert_eq!(hard_limits_if_any(&root, "other").unwrap(), None);
    std::fs::write(root.join("projects/demo.md"), "# demo\n\nText.\n").unwrap();
    assert_eq!(hard_limits_if_any(&root, "demo").unwrap(), None);
}

#[test]
fn a_file_that_cannot_be_read_is_an_error_not_a_project_without_limits() {
    let (_scratch, root) = root_with("demo", FILE);
    std::fs::write(root.join("projects/demo.md"), [0xff, 0xfe, 0xfd]).unwrap();
    let error = hard_limits_if_any(&root, "demo").unwrap_err().text();
    assert!(error.contains("projects/demo.md"), "{error}");
}
