use super::*;
use crate::declared::Marks;
use std::path::PathBuf;

fn declared(name: &str) -> Declared {
    Declared {
        name: Name::parse(name).unwrap(),
        file: PathBuf::from("Shots.kt"),
        marks: Marks::default(),
    }
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn previews(root: &Path) -> Previews {
    Previews::of(root).unwrap()
}

fn calls() -> Vec<String> {
    vec!["N".to_string(), "W".to_string()]
}

fn sentences(root: &Path, shots: &[Declared]) -> Vec<String> {
    problems(root, &calls(), &previews(root), shots)
        .into_iter()
        .map(|p| p.sentence)
        .collect()
}

#[test]
fn preview_rows_are_read_as_state_ids() {
    let html = r#"W('Label', x), N('Nested, off', y), W('It\'s on', z), h('div')"#;
    assert_eq!(states(html, &calls()), ["label", "nested-off", "it-s-on"]);
}

#[test]
fn a_component_shot_needs_a_row_of_that_name() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "design/system/components/MacRow/preview.html",
        "W('Label', a), W('Nested, off', b)",
    );
    write(
        root.path(),
        "design/system/components/AndroidCastRow/preview.html",
        "<p>no rows</p>",
    );
    let shots = [
        declared("mac.component.row.label"),
        declared("mac.component.row.nested-off"),
        declared("mac.component.row.missing"),
        declared("android.component.cast-row.default"),
        declared("android.component.cast-row.looking"),
        declared("android.component.nothing.default"),
    ];
    let problems = sentences(root.path(), &shots);
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(problems[0].contains("mac.component.row.missing"));
    assert!(problems[0].contains("label, nested-off"));
    assert!(problems[1].contains("android.component.cast-row.looking"));
    assert!(problems[1].contains("default"));
    assert!(problems[2].contains("AndroidNothing/preview.html"));
}

#[test]
fn a_shared_component_shot_needs_a_row_of_that_name_in_the_shared_preview() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "design/system/components/StatusChip/preview.html",
        "W('Done', a), W('Failed', b)",
    );
    let shots = [
        declared("mac.component.status-chip.done"),
        declared("android.component.status-chip.failed"),
        declared("android.component.status-chip.missing"),
    ];
    let problems = sentences(root.path(), &shots);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("android.component.status-chip.missing"));
    assert!(problems[0].contains("StatusChip has the states done, failed"));
}

#[test]
fn a_screen_shot_needs_its_board_file() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "design/mocks/mac.welcome.libraries.dc.html",
        "<x-dc></x-dc>",
    );
    let shots = [
        declared("mac.welcome.libraries"),
        declared("mac.welcome.create-name"),
    ];
    let problems = sentences(root.path(), &shots);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("design/mocks/mac.welcome.create-name.dc.html"));
}

#[test]
fn a_preview_it_cannot_read_says_why_and_not_that_it_is_missing() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "design/system/components/MacRow/preview.html",
        "",
    );
    std::fs::write(
        root.path()
            .join("design/system/components/MacRow/preview.html"),
        [0xff, 0xfe],
    )
    .unwrap();
    let problems = sentences(root.path(), &[declared("mac.component.row.label")]);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("cannot read"), "{problems:?}");
    assert!(!problems[0].contains("missing"), "{problems:?}");
}

#[test]
fn a_scene_the_harness_declares_with_no_board_is_a_problem() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "app/Shots/Scenes.swift",
        "ShotScene(id: \"mac.album.credits\", frame: .sheet) { }\n\
         ShotScene(id: \"mac.welcome.libraries\", frame: .sheet) { }",
    );
    write(
        root.path(),
        "design/mocks/mac.welcome.libraries.dc.html",
        "<x-dc></x-dc>",
    );
    let folders = [(crate::names::Platform::Mac, "app/Shots".to_string())];
    let declared = crate::declared::read(root.path(), &folders).unwrap();
    let found = sentences(root.path(), &declared);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("shot 'mac.album.credits' has no board"));
}
