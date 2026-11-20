use super::*;

const MAC: &str = "app/Shots";
const ANDROID: &str = "app/shots";

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn settings() -> Settings {
    Settings::parse(&format!(
        "shots-declared mac {MAC}\nshots-declared android {ANDROID}\nshots-preview-calls N W\n"
    ))
    .unwrap()
}

/// A repo whose components and harnesses hold one of each case: a component
/// whose capture file declares every row (AndroidRow), one whose file declares
/// some rows while a theme file declares the rest (MacRow), a shared
/// component with no capture file on either platform (Artwork), a web
/// component the repo has no web harness for (WebBox), and two capture files
/// with no preview on their platform.
fn repo() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path();
    write(
        root_path,
        "design/system/components/MacRow/preview.html",
        "W('Label', a), W('Nested, off', b)",
    );
    write(
        root_path,
        "design/system/components/AndroidRow/preview.html",
        "<p>no rows</p>",
    );
    write(
        root_path,
        "design/system/components/Artwork/preview.html",
        "N('Square', a)",
    );
    write(
        root_path,
        "design/system/components/WebBox/preview.html",
        "N('Plain', a)",
    );
    write(
        root_path,
        &format!("{MAC}/Components/MacRow.swift"),
        r#"ShotScene(id: "mac.component.row.label") { }"#,
    );
    write(
        root_path,
        &format!("{MAC}/ShotScenes.swift"),
        r#"ShotScene(id: "mac.component.row.nested-off") { }
           ShotScene(id: "mac.component.artwork.square") { }"#,
    );
    write(root_path, &format!("{MAC}/Components/MacGone.swift"), "");
    write(root_path, &format!("{MAC}/Components/AndroidRow.swift"), "");
    write(root_path, &format!("{MAC}/Components/Notes.txt"), "");
    write(
        root_path,
        &format!("{MAC}/Components/MacRowTests.swift"),
        "",
    );
    write(
        root_path,
        "design/system/components/row-old/preview.html",
        "N('Plain', a)",
    );
    write(
        root_path,
        &format!("{ANDROID}/Components/AndroidRow.kt"),
        r#"fun a() = component("android.component.row.default") {}"#,
    );
    root
}

fn listing(root: &Path) -> Listing {
    let settings = settings();
    let declared = crate::declared::read(root, &settings.declared).unwrap();
    list(root, &Previews::of(root).unwrap(), &settings, &declared).unwrap()
}

#[test]
fn each_component_with_a_preview_has_a_capture_file_on_each_platform_that_draws_it() {
    let root = repo();
    let found = listing(root.path());
    let summary: Vec<(String, Platform, PathBuf, bool)> = found
        .components
        .iter()
        .map(|c| (c.name.clone(), c.platform, c.file.clone(), c.file_exists))
        .collect();
    assert_eq!(
        summary,
        [
            (
                "AndroidRow".to_string(),
                Platform::Android,
                PathBuf::from("app/shots/Components/AndroidRow.kt"),
                true
            ),
            (
                "Artwork".to_string(),
                Platform::Mac,
                PathBuf::from("app/Shots/Components/Artwork.swift"),
                false
            ),
            (
                "Artwork".to_string(),
                Platform::Android,
                PathBuf::from("app/shots/Components/Artwork.kt"),
                false
            ),
            (
                "MacRow".to_string(),
                Platform::Mac,
                PathBuf::from("app/Shots/Components/MacRow.swift"),
                true
            ),
        ]
    );
}

#[test]
fn a_preview_row_counts_as_shot_only_when_its_components_capture_file_declares_it() {
    let root = repo();
    let found = listing(root.path());
    let rows = |name: &str, platform| -> Vec<(String, Place)> {
        found
            .components
            .iter()
            .find(|c| c.name == name && c.platform == platform)
            .unwrap()
            .rows
            .iter()
            .map(|r| (r.shot.to_string(), r.place.clone()))
            .collect()
    };
    assert_eq!(
        rows("MacRow", Platform::Mac),
        [
            ("mac.component.row.label".to_string(), Place::File),
            (
                "mac.component.row.nested-off".to_string(),
                Place::Elsewhere(PathBuf::from("app/Shots/ShotScenes.swift"))
            ),
        ]
    );
    assert_eq!(
        rows("AndroidRow", Platform::Android),
        [("android.component.row.default".to_string(), Place::File)]
    );
    assert_eq!(
        rows("Artwork", Platform::Android),
        [(
            "android.component.artwork.square".to_string(),
            Place::Nowhere
        )]
    );
}

#[test]
fn a_capture_file_whose_component_has_no_preview_on_its_platform_is_listed() {
    let root = repo();
    let found = listing(root.path());
    let strays: Vec<(PathBuf, String)> = found
        .strays
        .iter()
        .map(|s| (s.file.clone(), s.why.clone()))
        .collect();
    assert_eq!(
        strays,
        [
            (
                PathBuf::from("app/Shots/Components/AndroidRow.swift"),
                "AndroidRow is a component of android, not of mac".to_string()
            ),
            (
                PathBuf::from("app/Shots/Components/MacGone.swift"),
                "no folder design/system/components/MacGone holds a preview.html".to_string()
            ),
        ]
    );
}

#[test]
fn the_listing_names_each_gap_under_its_component_and_ends_with_the_counts() {
    let root = repo();
    let text = lines(&listing(root.path()), &[]);
    assert_eq!(
        text,
        [
            "Artwork on mac: app/Shots/Components/Artwork.swift does not exist",
            "  mac.component.artwork.square: ShotScenes.swift declares it, not this file",
            "Artwork on android: app/shots/Components/Artwork.kt does not exist",
            "  android.component.artwork.square: no harness file declares it",
            "MacRow on mac: app/Shots/Components/MacRow.swift",
            "  mac.component.row.nested-off: ShotScenes.swift declares it, not this file",
            "Capture files with no preview:",
            "  app/Shots/Components/AndroidRow.swift: AndroidRow is a component of android, not of mac",
            "  app/Shots/Components/MacGone.swift: no folder design/system/components/MacGone holds a preview.html",
            "Preview folders whose name is no component name, so no capture file can be named for them:",
            "  design/system/components/row-old",
            "3 preview rows of 3 components have no shot in their capture files, 2 capture files have no preview, and 1 preview folder has no component name.",
        ]
    );
}

#[test]
fn a_named_component_shows_every_row_whether_its_file_declares_it_or_not() {
    let root = repo();
    let found = listing(root.path());
    assert_eq!(
        lines(&found, &["MacRow".to_string(), "AndroidRow".to_string()]),
        [
            "AndroidRow on android: app/shots/Components/AndroidRow.kt",
            "  android.component.row.default: this file declares it",
            "MacRow on mac: app/Shots/Components/MacRow.swift",
            "  mac.component.row.label: this file declares it",
            "  mac.component.row.nested-off: ShotScenes.swift declares it, not this file",
            "Capture files with no preview:",
            "  app/Shots/Components/AndroidRow.swift: AndroidRow is a component of android, not of mac",
            "1 preview row of 1 component has no shot in its capture file, and 1 capture file has no preview.",
        ]
    );
    assert_eq!(
        lines(&found, &["MacGone".to_string()]),
        [
            "Capture files with no preview:",
            "  app/Shots/Components/MacGone.swift: no folder design/system/components/MacGone holds a preview.html",
            "1 capture file has no preview.",
        ]
    );
    assert_eq!(
        lines(&found, &["Artwork".to_string()]).last().unwrap(),
        "2 preview rows of 2 components have no shot in their capture files."
    );
    assert!(check_names(&found, &["MacRow".to_string(), "MacGone".to_string()]).is_ok());
    let unknown = check_names(&found, &["MacNothing".to_string()]).unwrap_err();
    assert!(unknown.contains("MacNothing"), "{unknown}");
}

#[test]
fn a_repo_with_no_component_shots_to_keep_says_so() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(MAC)).unwrap();
    std::fs::create_dir_all(root.path().join(ANDROID)).unwrap();
    let found = listing(root.path());
    assert!(found.components.is_empty() && found.strays.is_empty() && found.unnamed.is_empty());
    assert_eq!(
        lines(&found, &[]),
        ["Each preview row has its shot in its component's capture file."]
    );
}
