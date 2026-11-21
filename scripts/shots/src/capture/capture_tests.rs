use super::*;

fn names(texts: &[&str]) -> Vec<Name> {
    texts.iter().map(|t| Name::parse(t).unwrap()).collect()
}

fn prefix(text: &str) -> Prefix {
    Prefix::parse(text).unwrap()
}

#[test]
fn no_prefix_selects_every_shot_of_the_platform() {
    let declared = names(&["mac.welcome.a", "mac.settings.b", "android.welcome.a"]);
    let chosen = selected(&declared, Platform::Mac, &[]).unwrap();
    assert_eq!(chosen, names(&["mac.welcome.a", "mac.settings.b"]));
}

#[test]
fn a_prefix_selects_the_shots_it_starts() {
    let declared = names(&["mac.welcome.a", "mac.settings.b", "android.welcome.a"]);
    let chosen = selected(&declared, Platform::Mac, &[prefix("mac.welcome.")]).unwrap();
    assert_eq!(chosen, names(&["mac.welcome.a"]));
}

#[test]
fn a_prefix_that_starts_no_declared_shot_fails() {
    let declared = names(&["mac.welcome.a"]);
    let error = selected(&declared, Platform::Mac, &[prefix("mac.welcom.")]).unwrap_err();
    assert!(matches!(error, CaptureError::UnknownPrefix(p) if p == "mac.welcom."));
}

#[test]
fn a_platform_with_no_selected_shot_fails() {
    let declared = names(&["mac.welcome.a", "android.welcome.a"]);
    let error = selected(&declared, Platform::Android, &[prefix("mac.welcome.")]).unwrap_err();
    assert!(matches!(
        error,
        CaptureError::NothingSelected(Platform::Android)
    ));
}

/// A shot's valid pair of files, drawn at 2 pixels to the point.
fn write_shot(dir: &Path, name: &str) {
    let name = Name::parse(name).unwrap();
    let list = crate::shots::tests::LIST.replace("mac.welcome.a", &name.to_string());
    crate::shots::tests::write_pair(dir, &name, &list, 800, 380);
}

fn write(dir: &Path, files: &[&str]) {
    for file in files {
        std::fs::write(dir.join(file), "x").unwrap();
    }
}

#[test]
fn a_capture_is_complete_when_every_selected_shot_wrote_both_files() {
    let out = tempfile::tempdir().unwrap();
    write_shot(out.path(), "mac.welcome.a");
    let declared = names(&["mac.welcome.a", "mac.welcome.b"]);
    check_complete(out.path(), Platform::Mac, &declared[..1], &declared).unwrap();
}

#[test]
fn a_selected_shot_with_a_file_missing_fails() {
    let out = tempfile::tempdir().unwrap();
    write(out.path(), &["mac.welcome.a@macos.png"]);
    let declared = names(&["mac.welcome.a"]);
    let CaptureError::Incomplete(problems) =
        check_complete(out.path(), Platform::Mac, &declared, &declared).unwrap_err()
    else {
        panic!("not incomplete");
    };
    assert_eq!(
        problems,
        ["missing: the harness wrote no mac.welcome.a@macos.json"]
    );
}

#[test]
fn a_shot_file_no_harness_declares_fails() {
    let out = tempfile::tempdir().unwrap();
    write_shot(out.path(), "mac.welcome.a");
    write(
        out.path(),
        &[
            "mac.welcome.old@macos.png",
            "Not A Name@macos.json",
            "notes.txt",
            "android.welcome.a@android.png",
        ],
    );
    let declared = names(&["mac.welcome.a"]);
    let CaptureError::Incomplete(problems) =
        check_complete(out.path(), Platform::Mac, &declared, &declared).unwrap_err()
    else {
        panic!("not incomplete");
    };
    assert_eq!(
        problems,
        [
            "undeclared: Not A Name@macos.json is no declared shot",
            "undeclared: mac.welcome.old@macos.png is no declared shot",
        ]
    );
}

#[test]
fn clearing_empties_the_platforms_shot_files_and_leaves_other_files() {
    let out = tempfile::tempdir().unwrap();
    write(
        out.path(),
        &[
            "mac.welcome.a@macos.png",
            "mac.welcome.a@macos.json",
            "mac.welcome.left-by-another-branch@macos.png",
            "Not A Name@macos.json",
            "android.welcome.a@android.png",
            "notes.txt",
        ],
    );
    clear(out.path(), Platform::Mac).unwrap();
    let mut left: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["android.welcome.a@android.png", "notes.txt"]);
}

#[test]
fn a_shot_another_branch_left_behind_is_not_undeclared_after_clearing() {
    let out = tempfile::tempdir().unwrap();
    write_shot(out.path(), "mac.welcome.old");
    clear(out.path(), Platform::Mac).unwrap();
    write_shot(out.path(), "mac.welcome.a");
    let declared = names(&["mac.welcome.a"]);
    check_complete(out.path(), Platform::Mac, &declared, &declared).unwrap();
}

#[test]
fn a_shot_that_does_not_read_back_fails() {
    let out = tempfile::tempdir().unwrap();
    write_shot(out.path(), "mac.welcome.a");
    write_shot(out.path(), "mac.welcome.b");
    std::fs::write(out.path().join("mac.welcome.b@macos.json"), "{").unwrap();
    let declared = names(&["mac.welcome.a", "mac.welcome.b"]);
    let CaptureError::Incomplete(problems) =
        check_complete(out.path(), Platform::Mac, &declared, &declared).unwrap_err()
    else {
        panic!("not incomplete");
    };
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].starts_with("unreadable: mac.welcome.b: mac.welcome.b@macos.json:"));
}

#[test]
fn a_shot_whose_files_disagree_fails() {
    let out = tempfile::tempdir().unwrap();
    write_shot(out.path(), "mac.welcome.a");
    let name = Name::parse("mac.welcome.a").unwrap();
    std::fs::write(
        out.path().join(name.file_name("png")),
        crate::shots::tests::png(100, 100),
    )
    .unwrap();
    let declared = names(&["mac.welcome.a"]);
    let CaptureError::Incomplete(problems) =
        check_complete(out.path(), Platform::Mac, &declared, &declared).unwrap_err()
    else {
        panic!("not incomplete");
    };
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].starts_with("inconsistent: mac.welcome.a: its picture is 100x100"));
}

fn declared(texts: &[(&str, bool)]) -> Vec<Declared> {
    texts
        .iter()
        .map(|(t, needs_window)| Declared {
            name: Name::parse(t).unwrap(),
            file: PathBuf::from("Scenes.swift"),
            needs_window: *needs_window,
        })
        .collect()
}

#[test]
fn a_window_shot_is_not_taken_without_windows_and_taken_with_them() {
    let all = declared(&[("mac.welcome.a", false), ("mac.settings.toolbar", true)]);
    let without = taken(&all, Platform::Mac, &[], false).unwrap();
    assert_eq!(without.names, names(&["mac.welcome.a"]));
    assert_eq!(without.not_taken, names(&["mac.settings.toolbar"]));
    let with = taken(&all, Platform::Mac, &[], true).unwrap();
    assert_eq!(
        with.names,
        names(&["mac.welcome.a", "mac.settings.toolbar"])
    );
    assert!(with.not_taken.is_empty());
}

#[test]
fn a_selection_of_only_window_shots_is_all_not_taken_without_windows() {
    let all = declared(&[("mac.welcome.a", false), ("mac.settings.toolbar", true)]);
    let chosen = taken(&all, Platform::Mac, &[prefix("mac.settings.")], false).unwrap();
    assert!(chosen.names.is_empty());
    assert_eq!(chosen.not_taken, names(&["mac.settings.toolbar"]));
}

#[test]
fn the_completeness_check_passes_without_a_window_shot_when_it_was_not_taken() {
    let out = tempfile::tempdir().unwrap();
    write_shot(out.path(), "mac.welcome.a");
    let all = declared(&[("mac.welcome.a", false), ("mac.settings.toolbar", true)]);
    let chosen = taken(&all, Platform::Mac, &[], false).unwrap();
    let declared: Vec<Name> = all.iter().map(|d| d.name.clone()).collect();
    check_complete(out.path(), Platform::Mac, &chosen.names, &declared).unwrap();
}

#[test]
fn the_completeness_check_reports_a_window_shot_missing_when_windows_were_given() {
    let out = tempfile::tempdir().unwrap();
    write_shot(out.path(), "mac.welcome.a");
    let all = declared(&[("mac.welcome.a", false), ("mac.settings.toolbar", true)]);
    let chosen = taken(&all, Platform::Mac, &[], true).unwrap();
    let declared: Vec<Name> = all.iter().map(|d| d.name.clone()).collect();
    let CaptureError::Incomplete(problems) =
        check_complete(out.path(), Platform::Mac, &chosen.names, &declared).unwrap_err()
    else {
        panic!("not incomplete");
    };
    assert_eq!(
        problems,
        [
            "missing: the harness wrote no mac.settings.toolbar@macos.json",
            "missing: the harness wrote no mac.settings.toolbar@macos.png",
        ]
    );
}

/// A capture command that records the variables it ran with and copies the
/// shots in `ready` into its out folder.
fn copying_command(ready: &Path, seen: &Path) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "env | grep -E '^(TEST_RUNNER_)?BUILDS_SHOTS_(OUT|BOARD_SIZES|SELECT|WINDOWS)=' | sort > '{}' && cp '{}'/* \"$BUILDS_SHOTS_OUT\"",
            seen.display(),
            ready.display()
        ),
    ]
}

#[test]
fn a_capture_runs_the_apps_command_with_the_variables_and_checks_what_it_wrote() {
    let ready = tempfile::tempdir().unwrap();
    write_shot(ready.path(), "mac.welcome.a");
    let out = tempfile::tempdir().unwrap();
    write(out.path(), &["mac.welcome.stale@macos.png"]);
    let seen = tempfile::tempdir().unwrap();
    let seen = seen.path().join("env");
    let shots = declared(&[("mac.welcome.a", false), ("mac.welcome.window", true)]);
    let taken = capture(
        ready.path(),
        &copying_command(ready.path(), &seen),
        &shots,
        Platform::Mac,
        &[prefix("mac.welcome.")],
        false,
        &Target {
            out: out.path(),
            measure: &|_| Ok(BTreeMap::new()),
        },
    )
    .unwrap();
    assert_eq!(taken.names, names(&["mac.welcome.a"]));
    assert_eq!(taken.not_taken, names(&["mac.welcome.window"]));
    assert!(!out.path().join("mac.welcome.stale@macos.png").exists());
    let out_text = out.path().to_string_lossy();
    assert_eq!(
        std::fs::read_to_string(&seen)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            format!("BUILDS_SHOTS_BOARD_SIZES={out_text}/board-sizes.json"),
            format!("BUILDS_SHOTS_OUT={out_text}"),
            "BUILDS_SHOTS_SELECT=mac.welcome.".to_string(),
            format!("TEST_RUNNER_BUILDS_SHOTS_BOARD_SIZES={out_text}/board-sizes.json"),
            format!("TEST_RUNNER_BUILDS_SHOTS_OUT={out_text}"),
            "TEST_RUNNER_BUILDS_SHOTS_SELECT=mac.welcome.".to_string(),
        ]
    );
}

#[test]
fn a_capture_command_that_fails_before_any_shot_fails_the_capture() {
    let empty = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let shots = declared(&[("android.welcome.a", false)]);
    let run = |command: &[&str]| {
        let command: Vec<String> = command.iter().map(|w| w.to_string()).collect();
        capture(
            empty.path(),
            &command,
            &shots,
            Platform::Android,
            &[],
            true,
            &Target {
                out: out.path(),
                measure: &|_| Ok(BTreeMap::new()),
            },
        )
    };
    let failed = run(&["false"]).unwrap();
    assert_eq!(failed.failed.len(), 1);
    let wrote_nothing = run(&["true"]).unwrap();
    assert!(wrote_nothing.names.is_empty());
    assert_eq!(wrote_nothing.failed.len(), 1);
    assert!(matches!(run(&[]), Err(CaptureError::NoCommand("android"))));
}

#[test]
fn the_board_sizes_file_is_written_whole_before_the_command_starts() {
    let ready = tempfile::tempdir().unwrap();
    write_shot(ready.path(), "mac.checkbox.default");
    let out = tempfile::tempdir().unwrap();
    let seen = tempfile::tempdir().unwrap();
    let copy = seen.path().join("sizes");
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "cp \"$BUILDS_SHOTS_BOARD_SIZES\" '{}' && cp '{}'/* \"$BUILDS_SHOTS_OUT\"",
            copy.display(),
            ready.path().display()
        ),
    ];
    let name = names(&["mac.checkbox.default"]).remove(0);
    let sizes = BTreeMap::from([(
        name.clone(),
        Size {
            width: 110.0,
            height: 16.0,
        },
    )]);
    capture(
        ready.path(),
        &command,
        &declared(&[("mac.checkbox.default", false)]),
        Platform::Mac,
        &[],
        false,
        &Target {
            out: out.path(),
            measure: &|taken| {
                assert_eq!(taken, std::slice::from_ref(&name));
                Ok(sizes.clone())
            },
        },
    )
    .unwrap();
    let read: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&copy).unwrap()).unwrap();
    assert_eq!(
        read,
        serde_json::json!({"mac.checkbox.default": {"width": 110.0, "height": 16.0}})
    );
    assert!(!out.path().join(".board-sizes.json.tmp").exists());
}

#[test]
fn a_run_that_cannot_measure_the_boards_does_not_start_the_command() {
    let out = tempfile::tempdir().unwrap();
    let result = capture(
        out.path(),
        &["false".to_string()],
        &declared(&[("mac.checkbox.default", false)]),
        Platform::Mac,
        &[],
        false,
        &Target {
            out: out.path(),
            measure: &|_| Err("no browser".to_string()),
        },
    );
    assert!(matches!(result, Err(CaptureError::Measure(_))));
}

#[test]
fn one_failed_scene_among_several_leaves_the_others_taken_and_names_it() {
    let ready = tempfile::tempdir().unwrap();
    write_shot(ready.path(), "mac.welcome.a");
    write_shot(ready.path(), "mac.welcome.c");
    let out = tempfile::tempdir().unwrap();
    let shots = declared(&[
        ("mac.welcome.a", false),
        ("mac.welcome.b", false),
        ("mac.welcome.c", false),
    ]);
    // The harness writes the shots it could take, then exits 1.
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "cp '{}'/* \"$BUILDS_SHOTS_OUT\"; exit 1",
            ready.path().display()
        ),
    ];
    let taken = capture(
        ready.path(),
        &command,
        &shots,
        Platform::Mac,
        &[],
        false,
        &Target {
            out: out.path(),
            measure: &|_| Ok(BTreeMap::new()),
        },
    )
    .unwrap();
    assert_eq!(taken.names, names(&["mac.welcome.a", "mac.welcome.c"]));
    assert_eq!(taken.failed.len(), 1);
    assert_eq!(taken.failed[0].name, names(&["mac.welcome.b"])[0]);
    assert!(taken.failed[0].why.contains("failed"));
    assert!(taken.failed[0].why.contains("mac.welcome.b@macos.png"));
}
