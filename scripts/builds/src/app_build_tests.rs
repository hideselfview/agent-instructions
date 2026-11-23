use super::*;
use crate::app_config::{PickKind, parse_config};
use crate::app_timing::{StepKind, StepResult};
use crate::testing::Scratch;
use std::time::Duration;

/// A lock of an app lane in <scratch>'s places, held while a test runs steps,
/// which record the step they run beside it.
fn held_in(scratch: &Scratch) -> lock::Held {
    let places = scratch.places();
    places
        .write_app_checkouts(
            "steps",
            Lane::Mac,
            &[crate::lane::AppCheckout::by_person("side-1")],
        )
        .unwrap();
    lock::take(&places, "steps", Lane::Mac).unwrap()
}

fn config() -> Config {
    parse_config("step mac one ./one\nstep android two ./two\nstep mac three ./three\nstep android four ./four\n").unwrap()
}

fn names(steps: &[Step]) -> Vec<&str> {
    steps.iter().map(|step| step.name.as_str()).collect()
}

#[test]
fn no_step_named_runs_every_step_of_the_lane_in_the_order_written() {
    assert_eq!(
        names(&chosen(&config(), Lane::Mac, &[]).unwrap()),
        vec!["one", "three"]
    );
    assert_eq!(
        names(&chosen(&config(), Lane::Android, &[]).unwrap()),
        vec!["two", "four"]
    );
}

#[test]
fn steps_named_run_in_the_order_named() {
    let named = ["three".to_string(), "one".to_string()];
    assert_eq!(
        names(&chosen(&config(), Lane::Mac, &named).unwrap()),
        vec!["three", "one"]
    );
}

#[test]
fn steps_named_go_to_the_lanes_they_are_in_each_in_the_order_named() {
    let named: Vec<String> = ["four", "three", "two"].map(str::to_string).to_vec();
    assert_eq!(names_in(&config(), Lane::Mac, &named), vec!["three"]);
    assert_eq!(
        names_in(&config(), Lane::Android, &named),
        vec!["four", "two"]
    );
}

#[test]
fn a_lane_refuses_a_step_of_the_other_lane() {
    let Err(Fail::Message(message)) = chosen(&config(), Lane::Mac, &["two".to_string()]) else {
        panic!("refused")
    };
    assert!(
        message.contains("android lane, not the mac lane"),
        "{message}"
    );
}

#[test]
fn a_step_the_config_lacks_is_refused_with_the_ones_it_has() {
    let Err(Fail::Message(message)) = chosen(&config(), Lane::Mac, &["nope".to_string()]) else {
        panic!("refused")
    };
    assert!(
        message.contains("nope") && message.contains("one, two, three, four"),
        "{message}"
    );
}

#[test]
fn a_ci_only_step_named_is_refused_naming_the_rule_and_left_out_of_every_step() {
    let config =
        parse_config("step mac one ./one\nstep mac suite ./suite\nci-only suite\n").unwrap();
    let refused = chosen(
        &config,
        Lane::Mac,
        &["one".to_string(), "suite".to_string()],
    )
    .unwrap_err()
    .text();
    assert!(refused.contains("suite"), "{refused}");
    assert!(refused.contains(ONLY_WHAT_A_CHANGE_TOUCHES), "{refused}");
    assert_eq!(
        names(&chosen(&config, Lane::Mac, &[]).unwrap()),
        vec!["one"]
    );
}

#[test]
fn a_marker_line_names_the_checkout_in_the_prefix_and_other_lines_do_not() {
    let marker = format!("{CHECKOUT_MARKER}side-9\n");
    assert_eq!(
        prefix_from_marker("android", marker.as_bytes()).as_deref(),
        Some("[android side-9] ")
    );
    assert_eq!(
        prefix_from_marker("android", b"step android-bridge\n"),
        None
    );
    assert_eq!(prefix_from_marker("mac", &[0xff, 0xfe]), None);
}

#[test]
fn steps_stop_at_the_first_failure() {
    let scratch = crate::testing::Scratch::new("app-build-steps");
    let held = held_in(&scratch);
    let step = |name: &str, command: &[&str]| Step {
        name: name.to_string(),
        lane: Lane::Mac,
        command: command.iter().map(|w| w.to_string()).collect(),
        limit: None,
        scenes: None,
        place: lock::Where::Anywhere,
        window: None,
        ci_only: false,
    };
    let steps = [
        step("a", &["touch", "a"]),
        step("b", &["false"]),
        step("c", &["touch", "c"]),
    ];
    assert!(matches!(
        run_steps(
            &scratch.dir,
            &held,
            &steps,
            &mut Vec::new(),
            &any_store(&scratch),
            Path::new("no.xcconfig"),
            false
        ),
        Err(Fail::Exit(1))
    ));
    assert!(scratch.dir.join("a").exists());
    assert!(
        !scratch.dir.join("c").exists(),
        "the step after the failure did not run"
    );
    assert_eq!(
        run_steps(
            &scratch.dir,
            &held,
            &steps[..1],
            &mut Vec::new(),
            &any_store(&scratch),
            Path::new("no.xcconfig"),
            false
        )
        .unwrap(),
        Ended::Finished
    );
}

/// In a lane that builds with Xcode, a step that fails after xcodebuild said
/// it cannot start ends the run with what xcodebuild said, in place of the
/// step's failure. A step that prints the line and passes passed, and a step
/// that fails without it failed because of the code.
#[test]
fn a_step_that_fails_after_xcodebuild_said_it_cannot_start_is_the_hosts_failure() {
    let scratch = crate::testing::Scratch::new("app-build-xcode");
    let held = held_in(&scratch);
    let said = "A required plugin failed to load. Please ensure system content is up-to-date, try running 'xcodebuild -runFirstLaunch'.";
    let step = |script: String| Step {
        name: "xcframework".to_string(),
        lane: Lane::Mac,
        command: vec!["sh".to_string(), "-c".to_string(), script],
        limit: None,
        scenes: None,
        place: lock::Where::Anywhere,
        window: None,
        ci_only: false,
    };
    let broken = step(format!("echo building; echo \"{said}\" >&2; exit 70"));
    assert_eq!(
        run_steps(
            &scratch.dir,
            &held,
            std::slice::from_ref(&broken),
            &mut Vec::new(),
            &any_store(&scratch),
            Path::new("no.xcconfig"),
            true
        )
        .unwrap(),
        Ended::HostFault(Refused::Xcode, said.to_string())
    );
    assert!(
        matches!(
            run_steps(
                &scratch.dir,
                &held,
                &[broken],
                &mut Vec::new(),
                &any_store(&scratch),
                Path::new("no.xcconfig"),
                false
            ),
            Err(Fail::Exit(70))
        ),
        "a lane that does not watch for the line fails as the step did"
    );
    let passed = step(format!("echo \"{said}\"; exit 0"));
    assert_eq!(
        run_steps(
            &scratch.dir,
            &held,
            &[passed],
            &mut Vec::new(),
            &any_store(&scratch),
            Path::new("no.xcconfig"),
            true
        )
        .unwrap(),
        Ended::Finished
    );
    assert!(matches!(
        run_steps(
            &scratch.dir,
            &held,
            &[step("exit 65".to_string())],
            &mut Vec::new(),
            &any_store(&scratch),
            Path::new("no.xcconfig"),
            true
        ),
        Err(Fail::Exit(65))
    ));
}

#[test]
fn a_step_past_its_limit_is_killed_with_what_it_started_and_fails_the_build() {
    let scratch = crate::testing::Scratch::new("app-build-limit");
    let held = held_in(&scratch);
    let slow = Step {
        name: "slow".to_string(),
        lane: Lane::Mac,
        command: ["sh", "-c", "sleep 300 & echo $! > background; wait"]
            .map(str::to_string)
            .to_vec(),
        limit: Some(Duration::from_millis(500)),
        scenes: None,
        place: lock::Where::Anywhere,
        window: None,
        ci_only: false,
    };
    let after = Step {
        name: "after".to_string(),
        lane: Lane::Mac,
        command: vec!["touch".to_string(), "after".to_string()],
        limit: None,
        scenes: None,
        place: lock::Where::Anywhere,
        window: None,
        ci_only: false,
    };
    let started = std::time::Instant::now();
    let Err(Fail::Message(message)) = run_steps(
        &scratch.dir,
        &held,
        &[slow, after],
        &mut Vec::new(),
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    ) else {
        panic!("fails")
    };
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "killed at the limit, not when sleep ended"
    );
    assert!(
        message.contains("step slow") && message.contains("limit"),
        "{message}"
    );
    assert!(
        !scratch.dir.join("after").exists(),
        "the step after it did not run"
    );
    let background: i32 = std::fs::read_to_string(scratch.dir.join("background"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        unsafe { libc::kill(background, 0) },
        -1,
        "the step's background process was killed with its group"
    );
}

/// A shots step that runs <script> and counts <scenes> scenes of 400 ms
/// each, with a plain limit of 500 ms.
fn counted_shots(script: &str, scenes: usize) -> Step {
    let mut step = fake(
        "shots mac.",
        &["sh", "-c", script],
        Some(Duration::from_millis(500)),
    );
    step.scenes = Some(crate::app_config::Scenes {
        count: ["echo", &scenes.to_string()].map(str::to_string).to_vec(),
        each: Duration::from_millis(400),
    });
    step
}

#[test]
fn a_shots_run_longer_than_the_plain_limit_finishes_when_its_scenes_account_for_the_time() {
    let scratch = crate::testing::Scratch::new("app-build-scenes-limit");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let step = counted_shots("sleep 1.2", 3);
    run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    )
    .expect("1.2 s is within 500 ms and three scenes of 400 ms");
}

#[test]
fn a_local_step_runs_with_the_checkouts_xcode_settings_file() {
    let scratch = crate::testing::Scratch::new("app-build-xcode-settings");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let cached = shots_cache(&top, &places);
    let step = fake(
        "macos",
        &["sh", "-c", "echo \"$XCODE_XCCONFIG_FILE\" > seen"],
        None,
    );
    run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("/settings/side-1.xcconfig"),
        false,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(top.join("seen")).unwrap(),
        "/settings/side-1.xcconfig\n"
    );
}

#[test]
fn a_shots_run_that_hangs_is_killed_whatever_its_scenes() {
    let scratch = crate::testing::Scratch::new("app-build-scenes-hang");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let started = std::time::Instant::now();
    let Err(Fail::Message(message)) = run_steps(
        &top,
        &held,
        &[counted_shots("sleep 300", 3)],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    ) else {
        panic!("fails")
    };
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "killed at the limit"
    );
    assert!(message.contains("ran past its limit of"), "{message}");
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn test_and_shots_flags_follow_the_steps_each_naming_a_lane_and_an_argument() {
    let (names, picks) = split_picks(&args(&[
        "one",
        "two",
        "--test",
        "mac",
        "FooTests",
        "--shots",
        "android",
        "android.welcome.",
        "--test",
        "android",
        "BarTest",
    ]))
    .unwrap();
    assert_eq!(names, args(&["one", "two"]));
    assert_eq!(
        picks,
        vec![
            (Lane::Mac, PickKind::Class, "FooTests".to_string()),
            (
                Lane::Android,
                PickKind::Shots,
                "android.welcome.".to_string()
            ),
            (Lane::Android, PickKind::Class, "BarTest".to_string()),
        ]
    );
    assert_eq!(
        split_picks(&args(&["one"])).unwrap(),
        (args(&["one"]), Vec::new())
    );
}

#[test]
fn a_flag_needs_a_lane_and_an_argument_that_is_one_plain_word() {
    for bad in [
        &["--test"][..],
        &["--test", "mac"],
        &["--test", "ios", "X"],
        &["--test", "mac", "--run-only"],
        &["--test", "mac", "A B"],
        &["--test", "mac", ""],
        &["--test", "mac", "X", "stray"],
        &["--shots"],
        &["--shots", "mac"],
        &["--shots", "ios", "mac.x."],
        &["--shots", "mac", "--windows"],
        &["--shots", "mac", "a b"],
        &["--shots", "mac", "mac.x.", "stray"],
    ] {
        assert!(split_picks(&args(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn a_shots_prefix_that_selects_every_shot_of_its_lane_is_refused_naming_the_rule() {
    for (lane, prefix) in [
        ("mac", "mac."),
        ("android", "android."),
        ("mac", "mac"),
        ("mac", "m"),
    ] {
        let refused = split_picks(&args(&["one", "--shots", lane, prefix]))
            .unwrap_err()
            .text();
        assert!(refused.contains(prefix), "{refused}");
        assert!(refused.contains(ONLY_WHAT_A_CHANGE_TOUCHES), "{refused}");
        assert!(
            refused.contains(&format!("--all-shots {lane}")),
            "{refused}"
        );
    }
    assert_eq!(
        split_picks(&args(&["one", "--shots", "mac", "mac.import"]))
            .unwrap()
            .1,
        vec![(Lane::Mac, PickKind::Shots, "mac.import".to_string())]
    );
}

#[test]
fn all_shots_asks_a_lanes_process_for_every_shot_of_the_lane_and_reads_back_the_same() {
    let (names, picks) = split_picks(&args(&[
        "one",
        "--all-shots",
        "android",
        "--shots",
        "mac",
        "mac.import.",
    ]))
    .unwrap();
    assert_eq!(names, args(&["one"]));
    assert_eq!(
        picks,
        vec![
            (Lane::Android, PickKind::Shots, "android.".to_string()),
            (Lane::Mac, PickKind::Shots, "mac.import.".to_string()),
        ]
    );
    let words: Vec<String> = picks
        .iter()
        .flat_map(pick_words)
        .map(str::to_string)
        .collect();
    assert_eq!(
        words,
        args(&["--all-shots", "android", "--shots", "mac", "mac.import."])
    );
    assert_eq!(split_picks(&words).unwrap(), (Vec::new(), picks));
    assert!(split_picks(&args(&["--all-shots"])).is_err());
    assert!(split_picks(&args(&["--all-shots", "ios"])).is_err());
}

#[test]
fn a_lanes_full_set_of_shots_runs_through_its_shots_line_and_is_cached_as_a_shots_run() {
    let config =
        parse_config("step mac one ./one\nshots mac ./s --windows --select {select}\n").unwrap();
    let steps = lane_steps(
        &config,
        Lane::Mac,
        &[],
        &[all_shots(Lane::Mac)],
        &[],
        &|_| Ok(None),
    )
    .unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].name, "shots mac.");
    assert_eq!(
        steps[0].command,
        args(&["./s", "--windows", "--select", "mac."])
    );
    assert_eq!(pick_kind_of(&steps[0].name), Some(PickKind::Shots));
    assert_eq!(
        steps[0].scenes, None,
        "./s is not the checker, so none counts its scenes"
    );
}

#[test]
fn a_lanes_full_run_of_the_checker_counts_its_scenes_for_its_limit() {
    let config =
        parse_config("shots mac builds shots run --platform mac --windows --select {select}\n")
            .unwrap();
    let steps = lane_steps(
        &config,
        Lane::Mac,
        &[],
        &[all_shots(Lane::Mac)],
        &[],
        &|_| Ok(None),
    )
    .unwrap();
    let scenes = steps[0].scenes.as_ref().expect("counts its scenes");
    assert_eq!(
        scenes.count,
        args(&[
            "builds",
            "shots",
            "scenes",
            "--platform",
            "mac",
            "--windows",
            "--select",
            "mac."
        ])
    );
    assert_eq!(Some(scenes.each), PickKind::Shots.scene_time());
    assert!(scenes.limit_for(steps[0].limit(), 702).unwrap() > PickKind::Shots.limit() * 2);
}

#[test]
fn a_lane_runs_its_steps_then_each_test_line_once_and_only_the_picks_when_no_step_is_named() {
    let config = parse_config(
        "step mac one ./one\ntest mac ./t {class}\ntest android ./g {class}\nshots mac ./s --select {select}\n",
    )
    .unwrap();
    let picks = [
        (Lane::Mac, PickKind::Class, "A".to_string()),
        (Lane::Android, PickKind::Class, "B".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.welcome.".to_string()),
        (Lane::Mac, PickKind::Class, "C".to_string()),
    ];
    let steps = lane_steps(&config, Lane::Mac, &args(&["one"]), &picks, &[], &|_| {
        Ok(None)
    })
    .unwrap();
    assert_eq!(names(&steps), vec!["one", "test A C", "shots mac.welcome."]);
    assert_eq!(steps[1].command, args(&["./t", "A", "C"]));
    assert_eq!(steps[1].limit(), PickKind::Class.limit());
    assert_eq!(steps[2].command, args(&["./s", "--select", "mac.welcome."]));
    assert_eq!(steps[2].limit(), PickKind::Shots.limit());
    let only = lane_steps(&config, Lane::Mac, &[], &picks, &[], &|_| Ok(None)).unwrap();
    assert_eq!(names(&only), vec!["test A C", "shots mac.welcome."]);
    let android = lane_steps(&config, Lane::Android, &[], &picks, &[], &|_| Ok(None)).unwrap();
    assert_eq!(names(&android), vec!["test B"]);
    assert_eq!(
        names(&lane_steps(&config, Lane::Mac, &[], &[], &[], &|_| Ok(None)).unwrap()),
        vec!["one"]
    );
}

#[test]
fn a_lane_without_the_line_refuses_a_pick() {
    let config = parse_config("step mac one ./one\ntest mac ./t {class}\n").unwrap();
    let Err(Fail::Message(message)) = lane_steps(
        &config,
        Lane::Mac,
        &[],
        &[(Lane::Mac, PickKind::Shots, "mac.x.".to_string())],
        &[],
        &|_| Ok(None),
    ) else {
        panic!("refused")
    };
    assert!(message.contains("no `shots mac ...` line"), "{message}");
    let Err(Fail::Message(message)) = lane_steps(
        &config,
        Lane::Android,
        &[],
        &[(Lane::Android, PickKind::Class, "A".to_string())],
        &[],
        &|_| Ok(None),
    ) else {
        panic!("refused")
    };
    assert!(message.contains("no `test android ...` line"), "{message}");
}

#[test]
fn a_class_runs_through_the_program_its_folder_names() {
    let config = parse_config(
        "step mac one ./one\nsources mac:shots shots\ntest mac ./t {class}\ntest mac:shots ./s --only {class}\n",
    )
    .unwrap();
    let picks = [
        (Lane::Mac, PickKind::Class, "AppTests".to_string()),
        (Lane::Mac, PickKind::Class, "ShotTests".to_string()),
        (Lane::Mac, PickKind::Class, "ModelTests".to_string()),
        (Lane::Mac, PickKind::Class, "SceneTests".to_string()),
    ];
    let steps = lane_steps(&config, Lane::Mac, &[], &picks, &[], &|class| {
        Ok(matches!(class, "ShotTests" | "SceneTests").then(|| "shots".to_string()))
    })
    .unwrap();
    assert_eq!(
        names(&steps),
        vec!["test AppTests ModelTests", "test ShotTests SceneTests"]
    );
    assert_eq!(steps[0].command, args(&["./t", "AppTests", "ModelTests"]));
    assert_eq!(
        steps[1].command,
        args(&["./s", "--only", "ShotTests", "SceneTests"])
    );
}

#[test]
fn a_lane_with_one_shots_prefix_runs_it_in_one_step_that_captures() {
    let config = parse_config("shots mac ./s --select {select}\n").unwrap();
    let picks = [(Lane::Mac, PickKind::Shots, "mac.welcome.".to_string())];
    let steps = lane_steps(&config, Lane::Mac, &[], &picks, &[], &|_| Ok(None)).unwrap();
    assert_eq!(names(&steps), vec!["shots mac.welcome."]);
    assert_eq!(steps[0].command, args(&["./s", "--select", "mac.welcome."]));
}

#[test]
fn a_lane_with_several_shots_prefixes_captures_them_once_and_then_runs_each_without_capturing() {
    let config = parse_config(
        "step mac one ./one\ntest mac ./t {class}\nshots mac builds shots run --platform mac --windows --select {select}\n",
    )
    .unwrap();
    let picks = [
        (Lane::Mac, PickKind::Class, "A".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.welcome.".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.settings.".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.component.row.".to_string()),
    ];
    let windowed = [("mac.welcome.".to_string(), Access::Exclusive)];
    let steps = lane_steps(&config, Lane::Mac, &[], &picks, &windowed, &|_| Ok(None)).unwrap();
    assert_eq!(
        names(&steps),
        vec![
            "test A",
            "capture shots mac.welcome. mac.settings. mac.component.row.",
            "shots mac.welcome.",
            "shots mac.settings.",
            "shots mac.component.row.",
        ]
    );
    let capture = &steps[1];
    assert_eq!(
        capture.command,
        args(&[
            "builds",
            "shots",
            "run",
            "--platform",
            "mac",
            "--windows",
            "--select",
            "mac.welcome.",
            "--select",
            "mac.settings.",
            "--select",
            "mac.component.row.",
            "--capture-only",
        ])
    );
    assert!(is_capture(&capture.name));
    assert_eq!(pick_kind_of(&capture.name), None);
    assert_eq!(capture.limit(), PickKind::Shots.limit());
    let scenes = capture
        .scenes
        .as_ref()
        .expect("counts the scenes of every prefix");
    assert_eq!(
        scenes.count,
        args(&[
            "builds",
            "shots",
            "scenes",
            "--platform",
            "mac",
            "--windows",
            "--select",
            "mac.welcome.",
            "--select",
            "mac.settings.",
            "--select",
            "mac.component.row.",
        ]),
        "the count takes no flag that only a run has"
    );
    for (step, prefix) in
        steps[2..]
            .iter()
            .zip(["mac.welcome.", "mac.settings.", "mac.component.row."])
    {
        assert_eq!(
            step.command,
            args(&[
                "builds",
                "shots",
                "run",
                "--platform",
                "mac",
                "--windows",
                "--select",
                prefix,
                "--no-capture",
            ])
        );
        assert_eq!(
            step.scenes.as_ref().expect("counts its scenes").count,
            args(&[
                "builds",
                "shots",
                "scenes",
                "--platform",
                "mac",
                "--windows",
                "--select",
                prefix,
            ])
        );
        assert!(!is_capture(&step.name));
    }
    assert!(steps[0].window.is_none());
    assert!(
        capture.window.is_some(),
        "the capture takes the window scenes of the prefixes"
    );
    assert!(
        steps[2..].iter().all(|step| step.window.is_none()),
        "a run that captures nothing opens no window"
    );
}

#[test]
fn a_lane_whose_picture_folder_caches_every_shot_has_no_capture_step() {
    let config = parse_config("shots mac ./s --select {select}\n").unwrap();
    let picks = [
        (Lane::Mac, PickKind::Shots, "mac.a.".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.b.".to_string()),
    ];
    let steps = lane_steps_with(&config, Lane::Mac, &[], &picks, &[], true, &|_| Ok(None)).unwrap();
    assert_eq!(names(&steps), vec!["shots mac.a.", "shots mac.b."]);
    assert!(
        steps
            .iter()
            .all(|step| step.command.contains(&"--no-capture".to_string()))
    );
}

#[test]
fn a_single_prefix_read_from_a_caching_picture_folder_captures_nothing_and_takes_no_window() {
    let config = parse_config("shots mac ./s --windows --select {select}\n").unwrap();
    let picks = [(Lane::Mac, PickKind::Shots, "mac.a.".to_string())];
    let windowed = [("mac.a.".to_string(), Access::Exclusive)];
    let steps = lane_steps_with(&config, Lane::Mac, &[], &picks, &windowed, true, &|_| {
        Ok(None)
    })
    .unwrap();
    assert_eq!(names(&steps), vec!["shots mac.a."]);
    assert!(steps[0].command.contains(&"--no-capture".to_string()));
    assert!(steps[0].window.is_none());
}

#[test]
fn the_capture_of_prefixes_with_no_window_scene_opens_no_window() {
    let config = parse_config("shots mac ./s --windows --select {select}\n").unwrap();
    let picks = [
        (Lane::Mac, PickKind::Shots, "mac.component.a.".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.component.b.".to_string()),
    ];
    let steps = lane_steps(&config, Lane::Mac, &[], &picks, &[], &|_| Ok(None)).unwrap();
    assert!(steps.iter().all(|step| step.window.is_none()));
}

#[test]
fn a_shots_line_with_the_placeholder_beside_other_text_repeats_the_word_for_each_prefix() {
    let config = parse_config("shots mac ./s --select={select}\n").unwrap();
    let picks = [
        (Lane::Mac, PickKind::Shots, "mac.a.".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.b.".to_string()),
    ];
    let steps = lane_steps(&config, Lane::Mac, &[], &picks, &[], &|_| Ok(None)).unwrap();
    assert_eq!(
        steps[0].command,
        args(&[
            "./s",
            "--select=mac.a.",
            "--select=mac.b.",
            "--capture-only"
        ])
    );
}

#[test]
fn a_shots_line_that_puts_the_placeholder_after_no_flag_cannot_repeat_it_and_is_refused() {
    let config = parse_config("shots mac ./s {select}\n").unwrap();
    let picks = [
        (Lane::Mac, PickKind::Shots, "mac.a.".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.b.".to_string()),
    ];
    let Err(Fail::Message(message)) =
        lane_steps(&config, Lane::Mac, &[], &picks, &[], &|_| Ok(None))
    else {
        panic!("refused")
    };
    assert!(message.contains("{select}"), "{message}");
}

#[test]
fn a_test_run_that_opens_windows_may_take_any_checkout_and_holds_the_window_lock() {
    let config = parse_config(
        "step mac build ./b\nstep mac signed ./s\nhere signed\nsources mac:ui ui\ntest mac ./t {class}\ntest mac:ui ./t --ui {class}\nopens-windows mac:ui\n",
    )
    .unwrap();
    let program_of = |class: &str| Ok((class == "MenuTests").then(|| "ui".to_string()));
    let picks = [
        (Lane::Mac, PickKind::Class, "MenuTests".to_string()),
        (Lane::Mac, PickKind::Class, "ModelTests".to_string()),
    ];
    let steps = lane_steps(
        &config,
        Lane::Mac,
        &args(&["build"]),
        &picks,
        &[],
        &program_of,
    )
    .unwrap();
    assert!(steps[1].window.is_some());
    assert!(steps[2].window.is_none(), "a unit test class opens none");
    assert_eq!(place_of(&steps), lock::Where::Anywhere);
    let signed = lane_steps(
        &config,
        Lane::Mac,
        &args(&["signed"]),
        &[],
        &[],
        &program_of,
    )
    .unwrap();
    assert_eq!(place_of(&signed), lock::Where::Here);
    let both = lane_steps(
        &config,
        Lane::Mac,
        &args(&["signed"]),
        &picks,
        &[],
        &program_of,
    )
    .unwrap();
    assert_eq!(
        place_of(&both),
        lock::Where::Here,
        "the signed app and a window run share this Mac's checkout"
    );
}

#[test]
fn a_shots_run_holds_the_window_lock_for_each_prefix_that_selects_a_scene_shown_in_a_window() {
    let config = parse_config("shots mac ./s --windows --select {select}\n").unwrap();
    let windowed = [
        ("mac.welcome.".to_string(), Access::Exclusive),
        ("mac.settings.".to_string(), Access::Exclusive),
    ];
    let run = |prefix: &str| {
        let pick = (Lane::Mac, PickKind::Shots, prefix.to_string());
        lane_steps(&config, Lane::Mac, &[], &[pick], &windowed, &|_| Ok(None)).unwrap()
    };
    for prefix in ["mac.welcome.", "mac.settings."] {
        assert!(
            run(prefix)[0].window.is_some(),
            "every window scene adds a shot display to its host, so no two such runs share it"
        );
    }
    let component = run("mac.component.row.");
    assert!(
        component[0].window.is_none(),
        "a component run draws no window"
    );
    assert_eq!(place_of(&component), lock::Where::Anywhere);
}

#[test]
fn a_pick_step_is_known_by_its_name() {
    assert_eq!(pick_kind_of("test Foo"), Some(PickKind::Class));
    assert_eq!(pick_kind_of("shots mac.x."), Some(PickKind::Shots));
    assert_eq!(pick_kind_of("macos"), None);
}

fn built(commit: &str, steps: &[&str]) -> Built {
    Built {
        commit: commit.to_string(),
        steps: steps.iter().map(|step| step.to_string()).collect(),
    }
}

#[test]
fn a_class_run_of_the_commit_just_built_passes() {
    let last = built("abc123", &["macos", "macos-tests"]);
    Built::allows_class_run(Some(&last), "side-1", "my-branch", "abc123", "abc123").unwrap();
}

#[test]
fn a_class_run_of_another_commit_is_refused_naming_what_was_built() {
    let last = built("abc123", &["macos", "macos-tests"]);
    let error = Built::allows_class_run(Some(&last), "side-1", "my-branch", "def456", "abc123")
        .unwrap_err();
    let text = format!("{error:?}");
    assert!(
        text.contains("abc123") && text.contains("macos macos-tests") && text.contains("my-branch"),
        "{text}"
    );
}

#[test]
fn a_class_run_with_no_finished_build_or_a_moved_checkout_is_refused() {
    assert!(Built::allows_class_run(None, "side-1", "b", "abc123", "abc123").is_err());
    let last = built("abc123", &["macos"]);
    assert!(Built::allows_class_run(Some(&last), "side-1", "b", "abc123", "def456").is_err());
}

#[test]
fn what_was_built_is_kept_in_a_file_and_cleared_by_the_next_run() {
    let scratch = crate::testing::Scratch::new("app-built");
    let places = scratch.places();
    let file = places.app_built_file("demo", "side-1");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    assert_eq!(Built::read(&file).unwrap(), None);
    let last = built("abc123", &["macos", "macos-tests"]);
    last.write(&file).unwrap();
    assert_eq!(Built::read(&file).unwrap(), Some(last));
    Built::clear(&file).unwrap();
    assert_eq!(Built::read(&file).unwrap(), None);
    Built::clear(&file).unwrap();
}

fn fake(name: &str, command: &[&str], limit: Option<Duration>) -> Step {
    Step {
        name: name.to_string(),
        lane: Lane::Mac,
        command: command.iter().map(|w| w.to_string()).collect(),
        limit,
        scenes: None,
        place: lock::Where::Anywhere,
        window: None,
        ci_only: false,
    }
}

#[test]
fn running_fake_steps_times_each_one_with_its_kind_and_result() {
    let scratch = Scratch::new("timing-steps");
    let held = held_in(&scratch);
    let steps = [
        fake("build", &["true"], None),
        fake("test Foo", &["true"], None),
        fake("shots mac.a.", &["false"], None),
        fake("never", &["true"], None),
    ];
    let mut log = Vec::new();
    let _ = run_steps(
        &scratch.dir,
        &held,
        &steps,
        &mut log,
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    );
    let seen: Vec<(&str, StepKind, &str)> = log
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.result.word()))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("build", StepKind::Step, "passed"),
            ("test Foo", StepKind::Test, "passed"),
            ("shots mac.a.", StepKind::Shots, "failed"),
        ],
        "a step after the failure is not started, so it has no row"
    );
    assert!(log.iter().all(|s| s.started_at > 0));
}

/// A shots run whose pairs fail it has done its work: its findings are what
/// it was run for, and a merge base's runs, which compare with nothing, fail
/// on every pair with findings. So the lane runs every other shots prefix
/// and test class it was asked for, and fails at its end with the shots
/// run's status. A shots run that fails without failing pairs, as when it
/// cannot render, stops the lane as any failed step does.
#[test]
fn a_shots_run_whose_pairs_fail_lets_the_lane_run_the_rest() {
    let scratch = Scratch::new("shots-fail-goes-on");
    let held = held_in(&scratch);
    let pairs_fail = "echo 'builds shots: 1 pair(s) fail the run: mac.a.one' >&2; exit 1";
    let steps = [
        fake("shots mac.a.", &["sh", "-c", pairs_fail], None),
        fake("shots mac.b.", &["true"], None),
        fake("shots mac.c.", &["sh", "-c", "exit 2"], None),
        fake("test Never", &["true"], None),
    ];
    let mut log = Vec::new();
    let ran = run_steps(
        &scratch.dir,
        &held,
        &steps,
        &mut log,
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    );
    assert!(matches!(ran, Err(Fail::Exit(2))), "{ran:?}");
    let seen: Vec<(&str, &str)> = log
        .iter()
        .map(|s| (s.name.as_str(), s.result.word()))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("shots mac.a.", "failed"),
            ("shots mac.b.", "passed"),
            ("shots mac.c.", "failed"),
        ]
    );
    let steps = [
        fake("shots mac.a.", &["sh", "-c", pairs_fail], None),
        fake("test Foo", &["true"], None),
    ];
    let mut log = Vec::new();
    let ran = run_steps(
        &scratch.dir,
        &held,
        &steps,
        &mut log,
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    );
    assert!(matches!(ran, Err(Fail::Exit(1))), "{ran:?}");
    assert_eq!(log.len(), 2, "the test class ran after the shots run");
}

/// A step that fails keeps its excerpt and the shots its output says fail
/// the run; a step that passed keeps nothing.
#[test]
fn a_failed_step_keeps_what_its_output_said() {
    let scratch = Scratch::new("timing-step-failure");
    let held = held_in(&scratch);
    let said = "echo building; echo 'builds shots: 1 pair(s) fail the run: mac.a.one' >&2; exit 1";
    let mut log = Vec::new();
    let _ = run_steps(
        &scratch.dir,
        &held,
        &[
            fake("build", &["sh", "-c", "echo fine"], None),
            fake("shots mac.a.", &["sh", "-c", said], None),
        ],
        &mut log,
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    );
    assert_eq!(log[0].result, StepResult::Passed);
    let StepResult::Failed(failure) = log[1].result.clone() else {
        panic!("{:?}", log[1].result)
    };
    assert_eq!(failure.failed, vec!["mac.a.one"]);
    let excerpt = failure.excerpt.unwrap();
    assert!(
        excerpt.contains("building") && excerpt.contains("fail the run: mac.a.one"),
        "{excerpt}"
    );
}

/// Each step names itself in the record beside the lock as it starts: the
/// second step reads the record and finds its own name there.
#[test]
fn each_step_is_recorded_beside_the_lock_as_the_step_that_runs() {
    let scratch = Scratch::new("timing-running-step");
    let held = held_in(&scratch);
    let record = scratch
        .places()
        .app_lock_run_file("steps", Lane::Mac, &held.checkout);
    let names = format!("grep -q '\"name\":\"macos-tests\"' {}", record.display());
    run_steps(
        &scratch.dir,
        &held,
        &[
            fake("macos", &["true"], None),
            fake("macos-tests", &["sh", "-c", &names], None),
        ],
        &mut Vec::new(),
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    let run = crate::holder_run::HolderRun::read(&record)
        .unwrap()
        .unwrap();
    assert_eq!(
        (run.joined_at, run.waited_ms),
        (held.joined_at, held.waited.as_millis() as u64)
    );
    assert_eq!(run.step.unwrap().name, "macos-tests");
}

#[test]
fn a_fake_step_that_burns_cpu_records_cpu_time_and_one_past_its_limit_records_a_timeout() {
    let scratch = Scratch::new("timing-cpu");
    let held = held_in(&scratch);
    let burn = "i=0; while [ $i -lt 30000 ]; do i=$((i+1)); done";
    let mut log = Vec::new();
    run_steps(
        &scratch.dir,
        &held,
        &[fake("burn", &["sh", "-c", burn], None)],
        &mut log,
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    assert!(log[0].cpu_ms > 0, "{:?}", log[0]);
    assert!(log[0].wall_ms > 0);
    let mut log = Vec::new();
    let _ = run_steps(
        &scratch.dir,
        &held,
        &[fake(
            "slow",
            &["sh", "-c", "sleep 300"],
            Some(Duration::from_millis(300)),
        )],
        &mut log,
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    );
    assert_eq!(log[0].result.word(), "timed out");
}

#[test]
fn a_finished_lane_run_writes_its_wait_steps_and_checkout_to_its_projects_store() {
    let scratch = crate::testing::Scratch::new("timing-lane-run");
    let held = held_in(&scratch);
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let mut timing = Timing::start();
    timing.held = Some(HeldTimes {
        joined_at: 1_790_000_000,
        wait: Duration::from_millis(1500),
        checkout: "side-3".to_string(),
        requested_commit: "a".repeat(40),
        load: Some(crate::host::Load {
            cores: 14,
            one_minute: 20.0,
        }),
    });
    run_steps(
        &scratch.dir,
        &held,
        &[
            fake("build", &["true"], None),
            fake("test Foo", &["true"], None),
        ],
        &mut timing.steps,
        &any_store(&scratch),
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    timing
        .record(&places, "demo", Lane::Mac, "my-branch", &[], RunEnd::Passed)
        .unwrap();
    let path = store.dir.join(app_timing::DATABASE);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let row: (i64, i64, String, String, String, i64, f64) = conn
        .query_row(
            "SELECT started_at, wait_ms, checkout, built_ref, outcome, cores, load FROM app_build_runs",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        row,
        (
            1_790_000_000,
            1500,
            "side-3".to_string(),
            "my-branch".to_string(),
            "passed".to_string(),
            14,
            20.0
        )
    );
    let repo = &app_timing::stats(&path, 0).unwrap()[0];
    assert_eq!(repo.repo, "demo");
    let names: Vec<&str> = repo.lanes[0]
        .steps
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"build") && names.contains(&"Foo"));
}

#[test]
fn a_run_that_never_took_a_checkout_writes_nothing() {
    let scratch = crate::testing::Scratch::new("timing-no-checkout");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    Timing::start()
        .record(&places, "demo", Lane::Mac, "my-branch", &[], RunEnd::Failed)
        .unwrap();
    assert!(
        app_timing::stats(&store.dir.join(app_timing::DATABASE), 0)
            .unwrap()
            .is_empty()
    );
}

/// A build of a branch starts each lane's process with the commit it checks
/// out and the ref the caller named, which the run records as the ref it
/// built, so `builds land` finds the branch's builds by the branch's name.
#[test]
fn a_lane_is_told_the_ref_the_caller_named_beside_the_commit_it_builds() {
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let words = lane_words(
        "bae",
        Lane::Mac,
        (commit, "my-branch"),
        (false, false),
        (&["macos".to_string()], &[]),
    );
    assert_eq!(
        words[..7],
        [
            "app-build",
            "bae",
            "--lane",
            "mac",
            commit,
            "--built-ref",
            "my-branch"
        ]
    );
}

/// A git repo at <dir> with a `.app-build` that has a shots line, and itself
/// as its `origin`, fetched, so origin/main is its main.
fn shots_repo(dir: &std::path::Path) {
    crate::testing::repo(dir, "target\n");
    std::fs::write(
        dir.join(CONFIG),
        "step mac macos true\nshots mac true {select}\nshots-declared mac app/shots\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("app/shots")).unwrap();
    std::fs::write(
        dir.join("app/shots/Shots.swift"),
        "let scenes = [\"mac.a.one\"]\n",
    )
    .unwrap();
    crate::testing::git(dir, &["add", CONFIG, "app"]);
    crate::testing::git(dir, &["commit", "-q", "-m", "Add shots"]);
    crate::testing::git(
        dir,
        &["remote", "add", "origin", &dir.display().to_string()],
    );
    crate::testing::git(dir, &["fetch", "-q", "origin"]);
}

/// A shots cache for a run of steps that caches no shots run.
fn any_store(scratch: &crate::testing::Scratch) -> ShotsCache {
    a_store(&scratch.places())
}

fn shots_cache(dir: &std::path::Path, places: &Places) -> ShotsCache {
    shots_repo(dir);
    a_store(places)
}

/// The environment the shots of these tests are made in.
const ENVIRONMENT: &str = "26B131-17C529";

/// The shots cache of repo `demo`'s Mac lane in <places>.
fn a_store(places: &Places) -> ShotsCache {
    ShotsCache {
        places: places.clone(),
        repo: "demo".to_string(),
        lane: Lane::Mac,
        versions: crate::shots_program::VERSIONS.clone(),
        befores: Vec::new(),
        environment: ENVIRONMENT.to_string(),
        app_tree: "0123456789abcdef".to_string(),
        no_capture: false,
        retake: false,
        base_run: false,
        bases: [
            "mac.",
            "mac.a.",
            "mac.b.",
            "mac.c.",
            "mac.settings.",
            "mac.welcome.",
            "mac.x.",
        ]
        .map(|prefix| (prefix.to_string(), BaseRun::Saved))
        .to_vec(),
    }
}

#[test]
fn only_the_run_of_a_merge_base_passes_base_to_the_shots_program() {
    let scratch = crate::testing::Scratch::new("app-build-base-flag");
    let step = fake("shots-mac", &["builds", "shots", "run"], None);
    let mut store = any_store(&scratch);
    assert!(
        !store
            .command(&step, None, "pictures")
            .contains(&"--base".to_string())
    );
    store.base_run = true;
    assert!(
        store
            .command(&step, None, "pictures")
            .contains(&"--base".to_string())
    );
}

/// The key of a run of <prefix> on the Mac lane of repo `demo` at <commit>,
/// by this builds' shots checker.
fn shots_key(commit: &str, prefix: &str) -> shots_cache::Key {
    shots_cache::Key::new(
        "demo",
        commit,
        Lane::Mac,
        ENVIRONMENT,
        prefix,
        &crate::shots_program::VERSIONS,
    )
}

/// Keeps a run of <key> whose report holds a findings.json.
fn cache_with_records(places: &Places, key: &shots_cache::Key) -> shots_cache::Cached {
    let staging = shots_cache::stage(places, key).unwrap();
    std::fs::create_dir_all(staging.report()).unwrap();
    std::fs::write(staging.report().join(crate::shot_causes::FILE), "{}").unwrap();
    staging.finish(1, "FAIL x: y\n").unwrap()
}

const FINDINGS_RUN: &str = "echo '   Compiling x' >&2; echo '    Running `target/debug/bae-shots run`' >&2; echo 'FAIL mac.a: missing' >&2; echo 'bae-shots: 1 pair(s) fail the run: mac.a' >&2; exit 1";

#[test]
fn a_shots_run_that_printed_findings_is_cached_under_its_commit_platform_prefix_and_version() {
    let scratch = crate::testing::Scratch::new("shots-keep");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let step = fake("shots mac.a.", &["sh", "-c", FINDINGS_RUN], None);
    let result = run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    );
    assert!(matches!(result, Err(Fail::Exit(1))));
    let key = shots_key(&resolve(&top, "HEAD").unwrap(), "mac.a.");
    let cached = shots_cache::lookup(&places, &key).unwrap().expect("cached");
    assert_eq!(cached.exit_code, 1);
    assert_eq!(
        cached.findings, "FAIL mac.a: missing\nbae-shots: 1 pair(s) fail the run: mac.a\n",
        "the text after cargo's Running line"
    );
    for other in [
        shots_cache::Key {
            versions: crate::shots_program::Versions {
                comparison: "0".repeat(16),
                ..key.versions.clone()
            },
            ..key.clone()
        },
        shots_cache::Key {
            versions: crate::shots_program::Versions {
                capture: "0".repeat(16),
                ..key.versions.clone()
            },
            ..key
        },
    ] {
        assert!(shots_cache::lookup(&places, &other).unwrap().is_none());
    }
}

#[test]
fn a_shots_run_that_died_on_an_error_caches_nothing() {
    let scratch = crate::testing::Scratch::new("shots-error");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let step = fake(
        "shots mac.a.",
        &[
            "sh",
            "-c",
            "echo 'bae-shots: the simulator did not start' >&2; exit 1",
        ],
        None,
    );
    let _ = run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    );
    let key = shots_key(&resolve(&top, "HEAD").unwrap(), "mac.a.");
    assert!(shots_cache::lookup(&places, &key).unwrap().is_none());
    assert!(!shots_cache::root(&places).join("demo").exists());
}

#[test]
fn app_build_with_only_a_shots_pick_prints_a_cached_run_and_takes_no_lane() {
    let scratch = crate::testing::Scratch::new("shots-serve");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    places
        .write_app_checkouts(
            "demo",
            Lane::Mac,
            &[crate::lane::AppCheckout::by_person("side-1")],
        )
        .unwrap();
    let marker = places.root.join("demo/side-1");
    std::fs::create_dir_all(&marker).unwrap();
    std::fs::write(marker.join(".checkout"), top.display().to_string()).unwrap();
    let step = fake("shots mac.a.", &["sh", "-c", FINDINGS_RUN], None);
    let _ = run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    );
    let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    // Served from the store: no lane process starts (one would run this test
    // program as `builds`, and wait for a lock that does not exist).
    cmd_app_build(
        &scratch.tool(30),
        &args(&["demo", "HEAD", "--shots", "mac.a."]),
    )
    .unwrap();
    crate::shots_program::cmd_shots(
        &places,
        &args(&["findings", "demo", "HEAD", "mac", "mac.a."]),
    )
    .unwrap();
    let Err(Fail::Message(message)) = crate::shots_program::cmd_shots(
        &places,
        &args(&["findings", "demo", "HEAD", "mac", "mac.other."]),
    ) else {
        panic!("no run of that prefix is cached")
    };
    assert!(
        message.contains("no run of mac.other.") && message.contains("builds app-build demo HEAD"),
        "{message}"
    );
}

/// An app build that prints a cached shots run in place of running it counts
/// for `builds land` as the branch's passing app build: it prints only the
/// cached runs that a passing app build of the commit ran, which is the record
/// land reads, and runs the others.
#[test]
fn a_build_that_reuses_a_cached_run_leaves_the_passing_build_that_land_counts() {
    use crate::app_timing::{RunRecord, StepRecord};
    let scratch = crate::testing::Scratch::new("shots-serve-covers");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    places
        .write_app_checkouts(
            "demo",
            Lane::Mac,
            &[crate::lane::AppCheckout::by_person("side-1")],
        )
        .unwrap();
    let marker = places.root.join("demo/side-1");
    std::fs::create_dir_all(&marker).unwrap();
    std::fs::write(marker.join(".checkout"), top.display().to_string()).unwrap();
    let step = fake("shots mac.a.", &["sh", "-c", FINDINGS_RUN], None);
    let _ = run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    );
    let commit = resolve(&top, "HEAD").unwrap();
    let store = crate::lists::fixtures::make_store_named(&places, "demo-project", &["demo"]);
    let pick: Pick = (Lane::Mac, PickKind::Shots, "mac.a.".to_string());
    let record = |outcome: RunEnd, shots: StepResult| {
        let step = |name: &str, kind, result| StepRecord {
            at_merge_base: None,
            name: name.to_string(),
            kind,
            started_at: 5,
            wall_ms: 1,
            cpu_ms: 1,
            result,
            shots: None,
        };
        let run = RunRecord {
            started_at: 5,
            repo: "demo".to_string(),
            lane: Lane::Mac,
            checkout: "side-1".to_string(),
            built_ref: "work".to_string(),
            requested_commit: None,
            built_commit: Some(commit.clone()),
            who: None,
            wait_ms: 0,
            outcome,
            load: None,
            steps: vec![
                step("macos", StepKind::Step, StepResult::Passed),
                step("shots mac.a.", StepKind::Shots, shots),
            ],
        };
        crate::app_timing::record(&store.database(), &run).unwrap();
    };
    let serve = || {
        serve_cached(
            &places,
            "demo",
            &commit,
            vec![pick.clone()],
            Path::new("/nonexistent/builds"),
        )
        .unwrap()
    };
    // No build of the commit ran the prefix: the cached run is not enough.
    assert_eq!(serve().pending, vec![pick.clone()]);
    record(RunEnd::Failed, StepResult::Failed(Default::default()));
    assert_eq!(serve().pending, vec![pick.clone()]);
    // A passing build ran it, so printing the cached run leaves a build that
    // land counts as covering the branch.
    record(RunEnd::Passed, StepResult::Passed);
    assert_eq!(serve().pending, vec![pick.clone()]);
    let report = report_run(&crate::shots_program::VERSIONS);
    let step = fake("shots mac.a.", &["sh", "-c", &report], None);
    run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    assert!(serve().pending.is_empty());
    let run = crate::app_timing::PassedRun {
        commit: commit.clone(),
        steps: vec!["shots mac.".to_string()],
    };
    assert!(run.ran_shots("mac.a.") && !run.ran_shots("android.a."));
    let config = parse_config("step mac macos ./macos.sh\non bae-macos/ macos\n").unwrap();
    let conn = crate::db::open(&store.database(), crate::db::Make::Never).unwrap();
    let coverage = crate::app_landing::needs(
        &config,
        &["bae-macos/a.swift".to_string()],
        &commit,
        &mut |lane| {
            crate::app_timing::passed_runs(
                &conn,
                "demo",
                lane,
                &store.database(),
                Some("work"),
                &[],
            )
        },
        &mut |_| Ok(vec!["mac.a.".to_string()]),
        &mut |_, _| Ok(Some(Vec::new())),
        &mut |_, commit| panic!("this prefix needs no declarations at {commit}"),
    )
    .unwrap();
    assert!(coverage.unbuilt.is_empty(), "{:?}", coverage.unbuilt);
}

/// A shots run that writes a report, with a findings.json that names
/// shots checker <versions>, and says where it is.
fn report_run(versions: &crate::shots_program::Versions) -> String {
    let (capture, version) = (&versions.capture, &versions.comparison);
    format!(
        "mkdir -p shots/report/img; echo png > shots/report/img/a.png; echo stale > shots/report/img/old.png; echo \"<img src=\\\"img/a.png\\\">\" > shots/report/index.html; echo '{{\"version\": \"{version}\", \"capture_version\": \"{capture}\", \"commit\": \"c\", \"scenes\": [], \"findings\": []}}' > shots/report/findings.json; echo '    Running `target/debug/builds-shots run`' >&2; echo 'builds shots: report at '\"$PWD\"'/shots/report/index.html' >&2; exit 0"
    )
}

#[test]
fn a_shots_run_caches_its_report_and_the_pictures_it_shows_under_its_key_in_the_cache() {
    let scratch = crate::testing::Scratch::new("shots-report");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let run = report_run(&crate::shots_program::VERSIONS);
    let step = fake("shots mac.a.", &["sh", "-c", &run], None);
    run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    let key = shots_key(&resolve(&top, "HEAD").unwrap(), "mac.a.");
    let cached = shots_cache::lookup(&places, &key).unwrap().expect("cached");
    assert!(
        cached.folder.starts_with(&places.root),
        "{:?}",
        cached.folder
    );
    let folder = cached.report();
    assert_eq!(
        std::fs::read_to_string(folder.join("img/a.png")).unwrap(),
        "png\n"
    );
    assert!(folder.join("index.html").exists());
    assert!(
        !folder.join("img/old.png").exists(),
        "only the pictures the page shows are cached"
    );
    // The next run in the checkout rewrites the checkout's folder; the cached
    // copy stays.
    std::fs::remove_dir_all(top.join("shots")).unwrap();
    assert!(folder.join("img/a.png").exists());
}

#[test]
fn a_report_whose_findings_name_another_shots_checker_caches_nothing() {
    let scratch = crate::testing::Scratch::new("shots-version");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let run = report_run(&crate::shots_program::Versions {
        comparison: "another".to_string(),
        ..crate::shots_program::VERSIONS.clone()
    });
    let step = fake("shots mac.a.", &["sh", "-c", &run], None);
    run_steps(
        &top,
        &held,
        &[step],
        &mut Vec::new(),
        &cached,
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    let key = shots_key(&resolve(&top, "HEAD").unwrap(), "mac.a.");
    assert!(shots_cache::lookup(&places, &key).unwrap().is_none());
    assert!(!shots_cache::run_folder(&places, &key).exists());
}

#[test]
fn a_shots_run_is_given_the_picture_folder_its_app_tree_and_its_before_runs_findings_when_its_prefix_has_one()
 {
    let scratch = crate::testing::Scratch::new("shots-before-arg");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let mut cached = shots_cache(&top, &places);
    let findings = scratch.dir.join("before.json");
    cached
        .befores
        .push(("mac.a.".to_string(), Compared::With(findings.clone())));
    let said = |name: &str| {
        let out = scratch.dir.join("args.txt");
        let step = fake(
            name,
            &[
                "sh",
                "-c",
                "printf '%s\\n' \"$@\" > \"$0\"",
                &out.display().to_string(),
            ],
            None,
        );
        run_steps(
            &top,
            &held,
            &[step],
            &mut Vec::new(),
            &cached,
            Path::new("no.xcconfig"),
            false,
        )
        .unwrap();
        std::fs::read_to_string(&out).unwrap()
    };
    let pictures = shots_cache::pictures(&places, "demo");
    assert_eq!(
        said("shots mac.a."),
        format!(
            "--before\n{}\n--pictures\n{}\n--app-tree\n0123456789abcdef\n",
            findings.display(),
            pictures.display()
        )
    );
    assert_eq!(
        said("shots mac.b."),
        format!(
            "--pictures\n{}\n--app-tree\n0123456789abcdef\n",
            pictures.display()
        )
    );
}

/// The pick of the shots of the Mac's a screens.
fn a_screens() -> Vec<Pick> {
    vec![(Lane::Mac, PickKind::Shots, "mac.a.".to_string())]
}

#[test]
fn a_lanes_shots_runs_compare_with_the_cached_run_of_the_merge_base_and_a_run_of_main_with_none() {
    let scratch = crate::testing::Scratch::new("shots-store-of");
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    shots_repo(&top);
    let main = resolve(&top, "HEAD").unwrap();
    let config = parse_config(&std::fs::read_to_string(top.join(CONFIG)).unwrap()).unwrap();
    let of = |commit: &str| {
        ShotsCache::of(
            &places,
            &top,
            "demo",
            Lane::Mac,
            ENVIRONMENT,
            commit,
            picture_tree(&top, commit, &config, Lane::Mac, ENVIRONMENT).unwrap(),
            &LanePick::of_lane(Lane::Mac, &a_screens()).unwrap(),
            false,
            false,
            false,
            &[],
            Path::new("/nonexistent/builds"),
        )
    };
    let on_main = of(&main).unwrap();
    assert!(
        on_main.befores.is_empty(),
        "a commit on origin/main compares with none"
    );
    crate::testing::git(&top, &["checkout", "-q", "-b", "work"]);
    crate::testing::git(&top, &["commit", "-q", "--allow-empty", "-m", "Work"]);
    let work = resolve(&top, "HEAD").unwrap();
    let Err(Fail::Message(message)) = of(&work) else {
        panic!("builds has no cached run of the merge base")
    };
    assert!(
        message.contains(&format!("builds app-build demo {main} --shots mac.a.")),
        "{message}"
    );
    let key = shots_key(&main, "mac.");
    let cached = cache_with_records(&places, &key);
    let on_work = of(&work).unwrap();
    assert_eq!(
        on_work.befores,
        vec![("mac.a.".to_string(), Compared::With(cached.findings_json()))]
    );
}

#[test]
fn a_prefix_that_starts_no_shot_declared_at_the_merge_base_compares_with_no_run() {
    let scratch = crate::testing::Scratch::new("shots-store-new-prefix");
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    shots_repo(&top);
    let config = parse_config(&std::fs::read_to_string(top.join(CONFIG)).unwrap()).unwrap();
    crate::testing::git(&top, &["checkout", "-q", "-b", "work"]);
    crate::testing::git(&top, &["commit", "-q", "--allow-empty", "-m", "Work"]);
    let work = resolve(&top, "HEAD").unwrap();
    let new = vec![(Lane::Mac, PickKind::Shots, "mac.new.".to_string())];
    let store = ShotsCache::of(
        &places,
        &top,
        "demo",
        Lane::Mac,
        ENVIRONMENT,
        &work,
        picture_tree(&top, &work, &config, Lane::Mac, ENVIRONMENT).unwrap(),
        &LanePick::of_lane(Lane::Mac, &new).unwrap(),
        false,
        false,
        false,
        &[],
        Path::new("/nonexistent/builds"),
    )
    .unwrap();
    let base = crate::derive::merge_base(&top, &work).unwrap();
    assert_eq!(
        store.befores,
        [("mac.new.".to_string(), Compared::Nothing(base.clone()))],
        "a prefix the base selects no scene for compares with an empty run"
    );
    assert_eq!(store.bases, [("mac.new.".to_string(), BaseRun::NotNeeded)]);
    let step = Step {
        name: "shots mac.new.".to_string(),
        lane: Lane::Mac,
        command: vec!["builds".to_string(), "shots".to_string(), "run".to_string()],
        limit: None,
        scenes: None,
        place: lock::Where::Anywhere,
        window: None,
        ci_only: false,
    };
    let command = store.local_command(&step);
    let at = command.iter().position(|word| word == "--empty-before");
    assert_eq!(
        at.and_then(|at| command.get(at + 1)),
        Some(&base),
        "the run is told its base holds nothing, so each pair is new: {command:?}"
    );
    assert!(!command.contains(&"--before".to_string()), "{command:?}");
}

#[test]
fn a_cached_run_made_in_another_environment_is_a_commits_own_run_and_is_printed() {
    let scratch = crate::testing::Scratch::new("shots-serve-environment");
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    shots_repo(&top);
    places
        .write_app_checkouts(
            "demo",
            Lane::Mac,
            &[crate::lane::AppCheckout::by_person("side-1")],
        )
        .unwrap();
    let marker = places.root.join("demo/side-1");
    std::fs::create_dir_all(&marker).unwrap();
    std::fs::write(marker.join(".checkout"), top.display().to_string()).unwrap();
    let commit = resolve(&top, "HEAD").unwrap();
    let program = Path::new("/nonexistent/builds");
    let serve = || serve_cached(&places, "demo", &commit, a_screens(), program).unwrap();
    assert_eq!(serve().pending, a_screens(), "no run is cached");
    let key = shots_cache::Key {
        environment: "25G241-27B52".to_string(),
        ..shots_key(&commit, "mac.a.")
    };
    cache_with_records(&places, &key);
    assert!(
        serve().pending.is_empty(),
        "a commit's own run is printed whatever environment made it"
    );
    let cached =
        shot_findings::cached_run(&places, "demo", "HEAD", Lane::Mac, "mac.a.", program).unwrap();
    assert_eq!(cached.key.environment, "25G241-27B52");
}

/// A request names its repo and ref; a lane's process, `--list` and other
/// commands name none.
#[test]
fn a_request_names_its_repo_and_ref() {
    let argv = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        requested(&argv(&[
            "/x/builds",
            "app-build",
            "bae",
            "work",
            "--test",
            "A"
        ])),
        Some(("bae", "work"))
    );
    assert_eq!(
        requested(&argv(&[
            "builds",
            "app-build",
            "bae",
            "--lane",
            "mac",
            "work"
        ])),
        None
    );
    assert_eq!(
        requested(&argv(&["builds", "app-build", "bae", "--list"])),
        None
    );
    assert_eq!(requested(&argv(&["builds", "cargo", "test", "x"])), None);
}

/// A lane's process names the commit it builds; a request and other commands
/// name none.
#[test]
fn a_lanes_process_names_its_repo_and_commit() {
    let argv = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        lane_requested(&argv(&[
            "/x/builds",
            "app-build",
            "bae",
            "--lane",
            "android",
            "abc123",
            "android-tests"
        ])),
        Some("abc123")
    );
    assert_eq!(
        lane_requested(&argv(&["builds", "app-build", "bae", "work"])),
        None
    );
    assert_eq!(
        lane_requested(&argv(&["builds", "app-build", "bae", "--lane", "mac"])),
        None
    );
    assert_eq!(lane_requested(&argv(&["builds", "lock", "bae"])), None);
}

#[test]
fn a_shots_run_of_a_lane_that_built_no_app_captures_none() {
    let scratch = crate::testing::Scratch::new("shots-no-capture");
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let mut cached = shots_cache(&top, &places);
    cached.no_capture = true;
    let step = fake("shots mac.a.", &["true"], None);
    let command = cached.local_command(&step);
    assert_eq!(command.last().map(String::as_str), Some("--no-capture"));
}

#[test]
fn a_shots_run_of_a_fresh_build_takes_every_shot_again() {
    let scratch = crate::testing::Scratch::new("shots-retake");
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let mut fresh = shots_cache(&top, &places);
    fresh.retake = true;
    let step = fake("shots mac.a.", &["true"], None);
    let command = fresh.local_command(&step);
    assert_eq!(command.last().map(String::as_str), Some("--retake"));
}

#[test]
fn the_picture_folder_caches_the_shots_of_the_prefixes_its_marks_cover_for_the_app_tree_and_environment()
 {
    let scratch = crate::testing::Scratch::new("shots-cached-lanes");
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    shots_repo(&top);
    let commit = resolve(&top, "HEAD").unwrap();
    let config = parse_config(&std::fs::read_to_string(top.join(CONFIG)).unwrap()).unwrap();
    let source_tree = crate::app_tree::of(&top, &commit, &config, Lane::Mac).unwrap();
    let tree = picture_tree(&top, &commit, &config, Lane::Mac, ENVIRONMENT).unwrap();
    assert_eq!(
        tree,
        crate::app_tree::in_environment(&source_tree, ENVIRONMENT)
    );
    let cached = |tree, prefixes: &[&str]| {
        let pictures = shots_cache::AppPictures::of(&places, "demo", Lane::Mac, tree);
        let prefixes: Vec<_> = prefixes
            .iter()
            .map(|prefix| shots::Prefix::parse(prefix).unwrap())
            .collect();
        let query = capture_query(&config, Lane::Mac, &prefixes).unwrap();
        pictures.while_held(Some(&query), |cached| cached).unwrap()
    };
    assert!(!cached(&tree, &["mac.a."]));
    let marks = shots::CaptureMarks::of(&shots_cache::app_picture_tree(
        &places,
        "demo",
        Lane::Mac,
        &tree,
    ));
    marks
        .write(&[shots::Prefix::parse("mac.").unwrap()], false)
        .unwrap();
    assert!(cached(&tree, &["mac.a."]) && cached(&tree, &["mac.a.", "mac.b."]));
    let other_environment = crate::app_tree::in_environment(&source_tree, "25G241-27B52");
    for tree in ["another-tree", &other_environment] {
        assert!(!cached(tree, &["mac.a."]));
    }
}

/// A global `--agent` before the subcommand hides neither a request nor a lane.
#[test]
fn an_agent_option_does_not_hide_a_build_request_or_lane() {
    let argv = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        requested(&argv(&[
            "builds",
            "--agent",
            "a1",
            "app-build",
            "bae",
            "work"
        ])),
        Some(("bae", "work"))
    );
    assert_eq!(
        lane_requested(&argv(&[
            "builds",
            "--agent",
            "a1",
            "app-build",
            "bae",
            "--lane",
            "mac",
            "abc"
        ])),
        Some("abc")
    );
}

/// A shots step that takes its shots from the picture folder prints this
/// line (scripts/shots/src/lib.rs `capture`).
const TOOK_FROM_FOLDER: &str = "echo 'builds shots: took the 12 mac shots of app tree t from the picture folder; capturing none' >&2";

/// A shots step's record says its environment, whether the run it compares
/// with was saved, and how many screenshots came from the picture folder and
/// how many were shot.
#[test]
fn a_shots_steps_record_says_its_base_run_and_its_screenshots() {
    let scratch = crate::testing::Scratch::new("shots-use-record");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let counted = |command: &str| {
        let mut step = fake("shots mac.a.", &["sh", "-c", command], None);
        step.scenes = Some(crate::app_config::Scenes {
            count: vec!["echo".to_string(), "9".to_string()],
            each: Duration::from_secs(5),
        });
        step
    };
    let mut log = Vec::new();
    run_steps(
        &top,
        &held,
        &[counted(TOOK_FROM_FOLDER)],
        &mut log,
        &cached,
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    run_steps(
        &top,
        &held,
        &[counted("true")],
        &mut log,
        &cached,
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    let used = |at: usize| log[at].shots.clone().expect("a shots step has its use");
    let pictures = |reused, captured| Some(Pictures { reused, captured });
    assert_eq!(used(0).environment, ENVIRONMENT);
    assert_eq!(used(0).base_run, BaseRun::Saved);
    assert_eq!(used(0).pictures, pictures(12, 0));
    assert_eq!(used(1).pictures, pictures(0, 9));
}

/// A step that is no shots step has no use, and a shots step that fails
/// before its run finishes counts no screenshots.
#[test]
fn only_a_shots_step_that_finished_counts_screenshots() {
    let scratch = crate::testing::Scratch::new("shots-use-none");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let mut log = Vec::new();
    let _ = run_steps(
        &top,
        &held,
        &[
            fake("macos", &["true"], None),
            fake("shots mac.a.", &["sh", "-c", "exit 3"], None),
        ],
        &mut log,
        &cached,
        Path::new("no.xcconfig"),
        false,
    );
    assert_eq!(log[0].shots, None);
    assert_eq!(log[1].shots.as_ref().map(|used| used.pictures), Some(None));
}

/// The lane's capture step counts the screenshots it shot, and the shots run
/// after it, which only reads them from the picture folder, counts none.
#[test]
fn the_capture_step_counts_the_screenshots_and_the_runs_after_it_count_none() {
    let scratch = crate::testing::Scratch::new("shots-use-capture");
    let held = held_in(&scratch);
    let places = scratch.places();
    let top = scratch.dir.join("repo");
    let cached = shots_cache(&top, &places);
    let counted = |name: &str, command: &str| {
        let mut step = fake(name, &["sh", "-c", command], None);
        step.scenes = Some(crate::app_config::Scenes {
            count: vec!["echo".to_string(), "9".to_string()],
            each: Duration::from_secs(5),
        });
        step
    };
    let mut log = Vec::new();
    run_steps(
        &top,
        &held,
        &[
            counted("capture shots mac.a. mac.b.", "true"),
            counted("shots mac.a.", TOOK_FROM_FOLDER),
        ],
        &mut log,
        &cached,
        Path::new("no.xcconfig"),
        false,
    )
    .unwrap();
    let used = |at: usize| log[at].shots.clone().expect("a picture step has its use");
    assert_eq!(used(0).base_run, BaseRun::NotNeeded);
    assert_eq!(
        used(0).pictures,
        Some(Pictures {
            reused: 0,
            captured: 9
        })
    );
    assert_eq!(used(1).pictures, None);
}

#[path = "app_build_pictures_tests.rs"]
mod pictures;
