use super::*;

const KOTLIN_DIRECTORY: &str = "android/shots";
const SWIFT_DIRECTORY: &str = "mac/Shots";

fn folders() -> Vec<(Platform, String)> {
    vec![
        (Platform::Android, KOTLIN_DIRECTORY.to_string()),
        (Platform::Mac, SWIFT_DIRECTORY.to_string()),
    ]
}

#[test]
fn kotlin_reads_the_first_argument_of_shot_and_component() {
    let source = r#"
        @Test
        fun androidWelcomeLibraries() = shot("android.welcome.libraries") { Welcome() }

        @Test
        fun androidComponentRowTitle() =
            component(
                "android.component.row.title",
                wide = true,
            ) { Row() }

        protected fun shot(board: String, content: @Composable () -> Unit) {}
        // "android.welcome.not-a-call"
    "#;
    let names: Vec<String> = kotlin(source).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        ["android.welcome.libraries", "android.component.row.title"]
    );
}

#[test]
fn swift_reads_string_literals_of_board_form() {
    let source = r#"
        welcomeWindow("mac.welcome.libraries.none", libraries: []),
        ShotScene(id: "mac.welcome.create-name", size: WelcomeWindow.size) {
        ShotScene(id: "appearance", size: size)
        let key = "macOS.version"
    "#;
    let names: Vec<String> = swift(source).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        ["mac.welcome.libraries.none", "mac.welcome.create-name"]
    );
}

#[test]
fn swift_marks_the_scenes_framed_in_a_window() {
    let source = r#"
        ShotScene(id: "mac.welcome.create-name", size: WelcomeWindow.size) {
            ShotStage(Text("a"))
        },
        ShotScene(
            id: "mac.settings.toolbar",
            size: CGSize(width: 600, height: 400),
            frame: .window(title: "Settings")
        ) {
            ShotStage(Text("b"))
        },
        welcomeWindow("mac.welcome.libraries.none", libraries: []),
        ShotScene(id: "mac.welcome.after", size: size) { ShotStage(Text("c")) },
    "#;
    assert_eq!(
        swift(source),
        [
            ("mac.welcome.create-name".to_string(), false),
            ("mac.settings.toolbar".to_string(), true),
            ("mac.welcome.libraries.none".to_string(), false),
            ("mac.welcome.after".to_string(), false),
        ]
    );
    assert_eq!(window_frames(source), 1);
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn read_joins_both_harnesses_and_leaves_the_tests_out() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &format!("{KOTLIN_DIRECTORY}/WelcomeShots.kt"),
        r#"fun a() = shot("android.welcome.libraries") {}"#,
    );
    write(
        root.path(),
        &format!("{KOTLIN_DIRECTORY}/ElementListTest.kt"),
        r#"fun a() = shot("android.test.known") {}"#,
    );
    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/ShotScene+Welcome.swift"),
        r#"ShotScene(id: "mac.welcome.create-name")
           ShotScene(id: "mac.welcome.create-name")"#,
    );
    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/ShotElementListTests.swift"),
        r#"list("mac.welcome.add-code.entering")"#,
    );
    let names: Vec<String> = read(root.path(), &folders())
        .unwrap()
        .iter()
        .map(|d| d.name.to_string())
        .collect();
    assert_eq!(
        names,
        ["mac.welcome.create-name", "android.welcome.libraries"]
    );
}

#[test]
fn read_carries_the_window_marker_and_refuses_a_frame_it_cannot_tie_to_an_id() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &format!("{KOTLIN_DIRECTORY}/WelcomeShots.kt"),
        "",
    );
    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/ShotScene+Settings.swift"),
        r#"ShotScene(id: "mac.settings.toolbar", size: size, frame: .window(title: "S")) { x }
           ShotScene(id: "mac.settings.plain", size: size) { y }"#,
    );
    let read_all = read(root.path(), &folders()).unwrap();
    let flags: Vec<(String, bool)> = read_all
        .iter()
        .map(|d| (d.name.to_string(), d.needs_window))
        .collect();
    assert_eq!(
        flags,
        [
            ("mac.settings.plain".to_string(), false),
            ("mac.settings.toolbar".to_string(), true)
        ]
    );

    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/ShotScene.swift"),
        "/// A scene is written `ShotScene(id: \"mac.x\", frame: .window(title: t))`.\n// frame: .window(\n",
    );
    assert_eq!(read(root.path(), &folders()).unwrap().len(), 2);

    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/ShotScene+Other.swift"),
        r#"ShotScene(id: id, size: size, frame: .window(title: "S")) { x }"#,
    );
    assert!(matches!(
        read(root.path(), &folders()),
        Err(DeclaredError::WindowScene(_))
    ));
}

#[test]
fn a_literal_that_is_not_a_board_name_fails_the_read() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &format!("{KOTLIN_DIRECTORY}/WelcomeShots.kt"),
        r#"fun a() = shot("android.welcome") {}"#,
    );
    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/ShotScene.swift"),
        "",
    );
    assert!(matches!(
        read(root.path(), &folders()),
        Err(DeclaredError::Name(..))
    ));
}

#[test]
fn a_web_shot_is_declared_by_its_fixture_file() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "web/shots/web.dashboard.page.json", "{}");
    write(root.path(), "web/shots/notes.txt", "");
    let found = read(root.path(), &[(Platform::Web, "web/shots".to_string())]).unwrap();
    let names: Vec<String> = found.iter().map(|d| d.name.to_string()).collect();
    assert_eq!(names, ["web.dashboard.page"]);
    write(root.path(), "web/shots/Not A Name.json", "{}");
    assert!(matches!(
        read(root.path(), &[(Platform::Web, "web/shots".to_string())]),
        Err(DeclaredError::Name(..))
    ));
}

#[test]
fn no_declare_folder_fails_the_read_and_does_not_read_nothing() {
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(
        read(root.path(), &[]),
        Err(DeclaredError::NoHarness)
    ));
}

#[test]
fn a_harness_with_a_scene_and_no_board_is_declared_for_the_lint_to_find() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "app/Shots/Scenes.swift",
        "ShotScene(id: \"mac.album.credits\", frame: .sheet) { }",
    );
    let found = read(root.path(), &[(Platform::Mac, "app/Shots".to_string())]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name.to_string(), "mac.album.credits");
}

#[test]
fn a_component_capture_file_declares_its_shots_from_the_components_folder() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/ShotScenes.swift"),
        r#"ShotScene(id: "mac.welcome.create-name")"#,
    );
    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/Components/MacRow.swift"),
        r#"ShotScene(id: "mac.component.row.label")
           ShotScene(id: "mac.component.row.nested-off")"#,
    );
    write(
        root.path(),
        &format!("{KOTLIN_DIRECTORY}/Components/AndroidRow.kt"),
        r#"fun a() = component("android.component.row.title") {}"#,
    );
    write(
        root.path(),
        &format!("{KOTLIN_DIRECTORY}/Components/AndroidRowTest.kt"),
        r#"fun a() = component("android.component.row.test") {}"#,
    );
    let found = read(root.path(), &folders()).unwrap();
    let names: Vec<(String, PathBuf)> = found
        .iter()
        .map(|d| (d.name.to_string(), d.file.clone()))
        .collect();
    let in_components =
        |folder: &str, file: &str| root.path().join(folder).join("Components").join(file);
    assert_eq!(
        names,
        [
            (
                "mac.welcome.create-name".to_string(),
                root.path().join(SWIFT_DIRECTORY).join("ShotScenes.swift")
            ),
            (
                "mac.component.row.label".to_string(),
                in_components(SWIFT_DIRECTORY, "MacRow.swift")
            ),
            (
                "mac.component.row.nested-off".to_string(),
                in_components(SWIFT_DIRECTORY, "MacRow.swift")
            ),
            (
                "android.component.row.title".to_string(),
                in_components(KOTLIN_DIRECTORY, "AndroidRow.kt")
            ),
        ]
    );
    assert_eq!(
        declared_in(
            Platform::Mac,
            &in_components(SWIFT_DIRECTORY, "MacRow.swift")
        )
        .unwrap()
        .iter()
        .map(|d| d.name.to_string())
        .collect::<Vec<_>>(),
        ["mac.component.row.label", "mac.component.row.nested-off"]
    );
}

#[test]
fn a_shot_two_files_declare_fails_the_read_naming_both() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/ShotScenes.swift"),
        r#"ShotScene(id: "mac.component.row.label") { }"#,
    );
    write(
        root.path(),
        &format!("{SWIFT_DIRECTORY}/Components/MacRow.swift"),
        r#"ShotScene(id: "mac.component.row.label") { }
           ShotScene(id: "mac.component.row.label") { }"#,
    );
    write(root.path(), &format!("{KOTLIN_DIRECTORY}/Shots.kt"), "");
    let error = read(root.path(), &folders()).unwrap_err().to_string();
    assert!(error.contains("mac.component.row.label"), "{error}");
    assert!(error.contains("ShotScenes.swift"), "{error}");
    assert!(error.contains("Components/MacRow.swift"), "{error}");
}
