use super::*;
use crate::app_config::parse_config;
use crate::testing::{Scratch, git, repo};

const CONFIG_TEXT: &str = "\
step mac macos ./m
step mac macos-tests ./mt
step android android-bridge ./ab
step android android ./a
on bae-macos/ macos-tests
on BaeKit/ macos-tests
on bae-android/ android-bridge android
on bae-bridge/ macos-tests android-bridge android
needs mac macos-tests
needs android android-bridge android
sources mac bae-macos BaeKit
sources android bae-android
test mac ./t {class}
";

fn config() -> Config {
    parse_config(CONFIG_TEXT).unwrap()
}

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|path| path.to_string()).collect()
}

fn names(plan: &[(Lane, Vec<String>)]) -> Vec<(&'static str, Vec<&str>)> {
    plan.iter()
        .map(|(lane, steps)| (lane.word(), steps.iter().map(String::as_str).collect()))
        .collect()
}

#[test]
fn a_swift_only_branch_builds_only_the_unsigned_mac_step() {
    let plan = plan(&config(), &paths(&["bae-macos/bae/View.swift"]), &[]);
    assert_eq!(names(&plan), vec![("mac", vec!["macos-tests"])]);
}

#[test]
fn a_kotlin_only_branch_builds_only_the_android_steps() {
    let plan = plan(&config(), &paths(&["bae-android/app/Screen.kt"]), &[]);
    assert_eq!(
        names(&plan),
        vec![("android", vec!["android-bridge", "android"])]
    );
}

#[test]
fn a_rust_only_branch_builds_nothing() {
    assert!(
        plan(
            &config(),
            &paths(&["bae-library/src/lib.rs", "spec/bae.md"]),
            &[]
        )
        .is_empty()
    );
}

#[test]
fn a_bridge_change_builds_both_lanes_with_each_step_once() {
    let plan = plan(
        &config(),
        &paths(&["bae-bridge/src/types.rs", "bae-macos/x.swift"]),
        &[],
    );
    assert_eq!(
        names(&plan),
        vec![
            ("mac", vec!["macos-tests"]),
            ("android", vec!["android-bridge", "android"])
        ]
    );
}

#[test]
fn a_pick_in_a_lane_the_branch_does_not_touch_builds_what_the_lane_needs() {
    let picks = vec![(Lane::Mac, PickKind::Class, "FooTests".to_string())];
    let plan = plan(&config(), &paths(&["bae-library/src/lib.rs"]), &picks);
    assert_eq!(names(&plan), vec![("mac", vec!["macos-tests"])]);
}

#[test]
fn a_pick_in_a_lane_with_no_needs_line_runs_with_no_build() {
    let config = parse_config("step mac a ./a\ntest mac ./t {class}\n").unwrap();
    let picks = vec![(Lane::Mac, PickKind::Class, "FooTests".to_string())];
    let plan = plan(&config, &[], &picks);
    assert_eq!(names(&plan), vec![("mac", vec![])]);
}

#[test]
fn a_shots_prefix_names_its_lane_and_a_class_must_be_in_exactly_one() {
    assert_eq!(lane_of_prefix("android.welcome.").unwrap(), Lane::Android);
    assert!(lane_of_prefix("welcome.").is_err());
    assert_eq!(lane_of_class(&[Lane::Mac], "X").unwrap(), Lane::Mac);
    assert!(lane_of_class(&[], "X").is_err());
    assert!(lane_of_class(&Lane::ALL, "X").is_err());
}

#[test]
fn the_flags_take_a_name_without_a_lane_and_fresh_anywhere() {
    let args: Vec<String> = ["--test", "FooTests", "--fresh", "--shots", "mac.a."]
        .iter()
        .map(|word| word.to_string())
        .collect();
    let (picks, fresh) = parse_picks(&args, |kind, arg| match kind {
        PickKind::Shots => lane_of_prefix(arg),
        PickKind::Class => Ok(Lane::Android),
    })
    .unwrap();
    assert!(fresh);
    assert_eq!(
        picks,
        vec![
            (Lane::Android, PickKind::Class, "FooTests".to_string()),
            (Lane::Mac, PickKind::Shots, "mac.a.".to_string())
        ]
    );
    assert!(parse_picks(&["--test".to_string()], |_, _| Ok(Lane::Mac)).is_err());
    assert!(parse_picks(&["mac".to_string()], |_, _| Ok(Lane::Mac)).is_err());
}

#[test]
fn the_changed_paths_are_those_after_the_merge_base_with_origin_main_and_a_class_is_found_in_its_lane()
 {
    let scratch = Scratch::new("check-diff");
    let top = scratch.dir.join("repo");
    repo(&top, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(&top, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&top, &["push", "-q", "origin", "main"]);
    git(&top, &["fetch", "-q", "origin"]);
    git(&top, &["checkout", "-q", "-b", "work"]);
    std::fs::create_dir_all(top.join("bae-macos")).unwrap();
    std::fs::write(
        top.join("bae-macos/FooTests.swift"),
        "final class FooTests {}\n",
    )
    .unwrap();
    git(&top, &["add", "bae-macos"]);
    git(&top, &["commit", "-q", "-m", "Add a test"]);
    assert_eq!(
        changed_paths(&top, "work").unwrap(),
        paths(&["bae-macos/FooTests.swift"])
    );
    let config = config();
    assert_eq!(
        lanes_declaring(&top, &config, "work", "FooTests").unwrap(),
        vec![Lane::Mac]
    );
    assert!(
        lanes_declaring(&top, &config, "work", "Nothing")
            .unwrap()
            .is_empty()
    );
}
