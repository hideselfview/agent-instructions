use super::*;
use crate::lane::Lane;
use crate::testing::{Scratch, git, repo};

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
            (lane, PickKind::Shots, prefix.to_string())
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
    ok.push((Lane::Mac, PickKind::Class, "FooTests".to_string()));
    refuse_undeclared(&top, "HEAD", &ok).unwrap();
    refuse_undeclared(&top, "HEAD", &[crate::app_build::all_shots(Lane::Mac)]).unwrap();
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
