use super::*;
use crate::app_config::parse_config;
use crate::lane::Lane;
use crate::testing::{Scratch, git, repo};
use crate::window_lock::Access;

const CONFIG_TEXT: &str = "\
shots-declared android bae-android/shots
shots-declared mac bae-macos/shots
";

const KOTLIN: &str = "\
fun shots() {
    shot(\"android.activity.list\")
    shot(\"android.activity.detail\")
    shot(\"android.settings.general\")
}
";

const SWIFT: &str = "\
let scenes = [\"mac.settings.general\", \"mac.settings.appearance-dark\", \"mac.welcome.start\"]
let window = ShotScene(id: \"mac.settings.window\", frame: .window(title: \"Settings\", titleBar: true)) { Text(\"x\") }
let alert = ShotScene(id: \"mac.settings.alert\", frame: .window(title: \"Settings\", titleBar: true), takesFocus: true) { Text(\"x\") }
";

/// A repo whose one commit declares the shots above.
fn declaring(scratch: &Scratch) -> std::path::PathBuf {
    let top = scratch.dir.join("repo");
    repo(&top, "target 1G\n");
    std::fs::write(top.join(".app-build"), CONFIG_TEXT).unwrap();
    for (folder, file, text) in [
        ("bae-android/shots", "Shots.kt", KOTLIN),
        ("bae-macos/shots", "Shots.swift", SWIFT),
    ] {
        std::fs::create_dir_all(top.join(folder)).unwrap();
        std::fs::write(top.join(folder).join(file), text).unwrap();
    }
    git(&top, &["add", "."]);
    git(&top, &["commit", "-q", "-m", "Declare shots"]);
    top
}

fn picks(prefixes: &[&str]) -> Vec<Pick> {
    prefixes
        .iter()
        .map(|prefix| {
            let lane = if prefix.starts_with("mac") {
                Lane::Mac
            } else {
                Lane::Android
            };
            Pick::Shots(lane, shots::Prefix::parse(prefix).unwrap())
        })
        .collect()
}

#[test]
fn a_prefix_that_starts_no_declared_shot_is_refused_naming_the_nearest_declared_prefixes() {
    let scratch = Scratch::new("shots-declared-refuse");
    let top = declaring(&scratch);
    let refused = refuse_undeclared(&top, "HEAD", &picks(&["android.activity.sheet."]))
        .unwrap_err()
        .text();
    assert!(refused.contains("android.activity.sheet."), "{refused}");
    assert!(refused.contains("android.activity.list"), "{refused}");
    assert!(refused.contains("android.activity.detail"), "{refused}");
    assert!(!refused.contains("android.settings"), "{refused}");
    let refused = refuse_undeclared(&top, "HEAD", &picks(&["mac.settings.look"]))
        .unwrap_err()
        .text();
    assert!(refused.contains("mac.settings.general"), "{refused}");
    assert!(
        refused.contains("mac.settings.appearance-dark"),
        "{refused}"
    );
}

#[test]
fn prefixes_that_start_a_declared_shot_pass_and_all_shots_needs_no_declared_name() {
    let scratch = Scratch::new("shots-declared-pass");
    let top = declaring(&scratch);
    let mut ok = picks(&[
        "android.activity.",
        "mac.settings.appearance",
        "mac.welcome.",
    ]);
    ok.push(Pick::Class(Lane::Mac, "FooTests".to_string()));
    refuse_undeclared(&top, "HEAD", &ok).unwrap();
    refuse_undeclared(&top, "HEAD", &[crate::app_build::all_shots(Lane::Mac)]).unwrap();
}

#[test]
fn a_prefix_that_selects_a_window_scene_is_named_and_one_that_selects_none_is_not() {
    let scratch = Scratch::new("shots-declared-windows");
    let top = declaring(&scratch);
    let mut asked = picks(&[
        "mac.settings.",
        "mac.settings.window",
        "mac.settings.alert",
        "mac.settings.general",
        "mac.welcome.",
        "android.activity.",
    ]);
    asked.push(Pick::Class(Lane::Mac, "FooTests".to_string()));
    asked.push(crate::app_build::all_shots(Lane::Mac));
    let config = parse_config("shared-shot-display mac\n").unwrap();
    assert_eq!(
        windowed_prefixes(&top, "HEAD", &config, &asked).unwrap(),
        [
            ("mac.settings.".to_string(), Access::Exclusive),
            ("mac.settings.window".to_string(), Access::Shared),
            ("mac.settings.alert".to_string(), Access::Exclusive),
            ("mac.".to_string(), Access::Exclusive),
        ],
    );
}

#[test]
fn a_capture_that_adds_a_display_for_bitmap_scenes_needs_the_window_lock() {
    let scratch = Scratch::new("shots-declared-own-display");
    let top = declaring(&scratch);
    let config = format!("{CONFIG_TEXT}shots mac ./capture --windows --select {{select}}\n");
    std::fs::write(top.join(".app-build"), &config).unwrap();
    git(&top, &["add", ".app-build"]);
    git(
        &top,
        &["commit", "-q", "-m", "Use a window-capable capture"],
    );
    assert_eq!(
        windowed_prefixes(
            &top,
            "HEAD",
            &parse_config(&config).unwrap(),
            &picks(&["mac.welcome."])
        )
        .unwrap(),
        [("mac.welcome.".to_string(), Access::Exclusive)]
    );
}

/// <declaring>'s repo with the Android harness's folder removed from the
/// commit, so the Android declarations cannot be read there.
fn without_android_folder(top: &std::path::Path) {
    git(top, &["rm", "-rq", "bae-android"]);
    git(top, &["commit", "-q", "-m", "Drop the Android harness"]);
}

#[test]
fn a_mac_pick_is_checked_against_the_mac_harness_alone() {
    let scratch = Scratch::new("shots-declared-lane-refuse");
    let top = declaring(&scratch);
    without_android_folder(&top);
    refuse_undeclared(&top, "HEAD", &picks(&["mac.settings.appearance"])).unwrap();
    let refused = refuse_undeclared(&top, "HEAD", &picks(&["mac.settings.look"]))
        .unwrap_err()
        .text();
    assert!(refused.contains("mac.settings.general"), "{refused}");
}

#[test]
fn a_prefix_is_checked_against_the_names_of_its_own_lane() {
    let scratch = Scratch::new("shots-declared-own-lane");
    let top = declaring(&scratch);
    let wrong_lane = vec![Pick::Shots(
        Lane::Android,
        shots::Prefix::parse("android.settings.").unwrap(),
    )];
    refuse_undeclared(&top, "HEAD", &wrong_lane).unwrap();
    let mac_named_android = vec![Pick::Shots(
        Lane::Mac,
        shots::Prefix::parse("android.activity.").unwrap(),
    )];
    assert!(refuse_undeclared(&top, "HEAD", &mac_named_android).is_err());
}

#[test]
fn a_mac_pick_needs_no_readable_android_harness_to_name_its_window_access() {
    let scratch = Scratch::new("shots-declared-lane-windows");
    let top = declaring(&scratch);
    without_android_folder(&top);
    let config = parse_config("shared-shot-display mac\n").unwrap();
    assert_eq!(
        windowed_prefixes(&top, "HEAD", &config, &picks(&["mac.settings.alert"])).unwrap(),
        [("mac.settings.alert".to_string(), Access::Exclusive)]
    );
}

#[test]
fn the_nearest_prefixes_start_at_the_longest_stem_some_shot_has() {
    let declared: Vec<String> = [
        "android.activity.detail",
        "android.activity.list.empty",
        "android.activity.list.full",
        "android.settings.general",
    ]
    .map(String::from)
    .to_vec();
    assert_eq!(
        nearest(&declared, "android.activity.sheet."),
        vec!["android.activity.detail", "android.activity.list."]
    );
    assert_eq!(
        nearest(&declared, "android.zzz."),
        vec!["android.activity.", "android.settings."]
    );
    assert!(nearest(&declared, "mac.welcome.").is_empty());
}
