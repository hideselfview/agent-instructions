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
fn the_limits_are_the_section_up_to_the_next_heading_of_its_level() {
    let (_scratch, root) = root_with("demo", FILE);
    assert_eq!(
        hard_limits_if_any(&root, "demo").unwrap(),
        "- Never do a.\n- Never do b.\n\n### Detail\n\nMore."
    );
}

#[test]
fn a_project_with_no_file_or_no_section_or_an_empty_one_has_no_limits() {
    let (_scratch, root) = root_with("demo", FILE);
    assert_eq!(hard_limits_if_any(&root, "other"), None);
    std::fs::write(root.join("projects/demo.md"), "# demo\n\nText.\n").unwrap();
    assert_eq!(hard_limits_if_any(&root, "demo"), None);
    std::fs::write(
        root.join("projects/demo.md"),
        "# demo\n\nText.\n\n## Hard limits\n\n## Next\n\nx\n",
    )
    .unwrap();
    assert_eq!(hard_limits_if_any(&root, "demo"), None);
}
