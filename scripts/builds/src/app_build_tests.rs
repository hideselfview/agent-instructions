use super::*;
use crate::app_config::{PickKind, parse_config};
use crate::app_timing::{StepKind, StepResult};
use crate::db;
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
        place: lock::Where::Anywhere,
        ci_only: false,
    };
    let steps = [
        step("a", &["touch", "a"]),
        step("b", &["false"]),
        step("c", &["touch", "c"]),
    ];
    assert!(matches!(
        run_steps(&scratch.dir, &held, &steps, &mut Vec::new(), None, false),
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
            None,
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
        place: lock::Where::Anywhere,
        ci_only: false,
    };
    let broken = step(format!("echo building; echo \"{said}\" >&2; exit 70"));
    assert_eq!(
        run_steps(
            &scratch.dir,
            &held,
            std::slice::from_ref(&broken),
            &mut Vec::new(),
            None,
            true
        )
        .unwrap(),
        Ended::HostFault(Refused::Xcode, said.to_string())
    );
    assert!(
        matches!(
            run_steps(&scratch.dir, &held, &[broken], &mut Vec::new(), None, false),
            Err(Fail::Exit(70))
        ),
        "a lane that does not watch for the line fails as the step did"
    );
    let passed = step(format!("echo \"{said}\"; exit 0"));
    assert_eq!(
        run_steps(&scratch.dir, &held, &[passed], &mut Vec::new(), None, true).unwrap(),
        Ended::Finished
    );
    assert!(matches!(
        run_steps(
            &scratch.dir,
            &held,
            &[step("exit 65".to_string())],
            &mut Vec::new(),
            None,
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
        place: lock::Where::Anywhere,
        ci_only: false,
    };
    let after = Step {
        name: "after".to_string(),
        lane: Lane::Mac,
        command: vec!["touch".to_string(), "after".to_string()],
        limit: None,
        place: lock::Where::Anywhere,
        ci_only: false,
    };
    let started = std::time::Instant::now();
    let Err(Fail::Message(message)) = run_steps(
        &scratch.dir,
        &held,
        &[slow, after],
        &mut Vec::new(),
        None,
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
    }
    assert_eq!(
        split_picks(&args(&["one", "--shots", "mac", "mac.import"]))
            .unwrap()
            .1,
        vec![(Lane::Mac, PickKind::Shots, "mac.import".to_string())]
    );
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
    let steps = lane_steps(&config, Lane::Mac, &args(&["one"]), &picks, &|_| Ok(None)).unwrap();
    assert_eq!(names(&steps), vec!["one", "test A C", "shots mac.welcome."]);
    assert_eq!(steps[1].command, args(&["./t", "A", "C"]));
    assert_eq!(steps[1].limit(), PickKind::Class.limit());
    assert_eq!(steps[2].command, args(&["./s", "--select", "mac.welcome."]));
    assert_eq!(steps[2].limit(), PickKind::Shots.limit());
    let only = lane_steps(&config, Lane::Mac, &[], &picks, &|_| Ok(None)).unwrap();
    assert_eq!(names(&only), vec!["test A C", "shots mac.welcome."]);
    let android = lane_steps(&config, Lane::Android, &[], &picks, &|_| Ok(None)).unwrap();
    assert_eq!(names(&android), vec!["test B"]);
    assert_eq!(
        names(&lane_steps(&config, Lane::Mac, &[], &[], &|_| Ok(None)).unwrap()),
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
    let steps = lane_steps(&config, Lane::Mac, &[], &picks, &|class| {
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
fn each_shots_prefix_runs_on_its_own() {
    let config = parse_config(
        "shots mac ./s --select {select}
",
    )
    .unwrap();
    let picks = [
        (Lane::Mac, PickKind::Shots, "mac.welcome.".to_string()),
        (Lane::Mac, PickKind::Shots, "mac.settings.".to_string()),
    ];
    let steps = lane_steps(&config, Lane::Mac, &[], &picks, &|_| Ok(None)).unwrap();
    assert_eq!(
        names(&steps),
        vec!["shots mac.welcome.", "shots mac.settings."]
    );
}

#[test]
fn a_test_run_that_opens_windows_takes_a_checkout_on_a_host_and_no_here_step() {
    let config = parse_config(
        "step mac build ./b\nstep mac signed ./s\nhere signed\nsources mac:ui ui\ntest mac ./t {class}\ntest mac:ui ./t --ui {class}\nopens-windows mac:ui\n",
    )
    .unwrap();
    let program_of = |class: &str| Ok((class == "MenuTests").then(|| "ui".to_string()));
    let picks = [
        (Lane::Mac, PickKind::Class, "MenuTests".to_string()),
        (Lane::Mac, PickKind::Class, "ModelTests".to_string()),
    ];
    let steps = lane_steps(&config, Lane::Mac, &args(&["build"]), &picks, &program_of).unwrap();
    assert_eq!(steps[1].place, lock::Where::Hosts);
    assert_eq!(steps[2].place, lock::Where::Anywhere);
    assert_eq!(place_of(&steps).unwrap(), lock::Where::Hosts);
    assert_eq!(
        place_of(&steps[2..]).unwrap(),
        lock::Where::Anywhere,
        "a unit test class runs anywhere"
    );
    let signed = lane_steps(&config, Lane::Mac, &args(&["signed"]), &[], &program_of).unwrap();
    assert_eq!(place_of(&signed).unwrap(), lock::Where::Here);
    let both = lane_steps(&config, Lane::Mac, &args(&["signed"]), &picks, &program_of).unwrap();
    let Err(Fail::Message(message)) = place_of(&both) else {
        panic!("no checkout runs both")
    };
    assert!(
        message.contains("signed runs only on this Mac")
            && message.contains("test MenuTests only on a host"),
        "{message}"
    );
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
        place: lock::Where::Anywhere,
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
    let _ = run_steps(&scratch.dir, &held, &steps, &mut log, None, false);
    let seen: Vec<(&str, StepKind, StepResult)> = log
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.result))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("build", StepKind::Step, StepResult::Passed),
            ("test Foo", StepKind::Test, StepResult::Passed),
            ("shots mac.a.", StepKind::Shots, StepResult::Failed),
        ],
        "a step after the failure is not started, so it has no row"
    );
    assert!(log.iter().all(|s| s.started_at > 0));
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
        None,
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
        None,
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
        None,
        false,
    );
    assert_eq!(log[0].result, StepResult::TimedOut);
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
        None,
        false,
    )
    .unwrap();
    timing.record(&places, "demo", Lane::Mac, "my-branch", true);
    let path = store.dir.join(app_timing::DATABASE);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let row: (i64, i64, String, String, bool, i64, f64) = conn
        .query_row(
            "SELECT started_at, wait_ms, checkout, built_ref, passed, cores, load FROM app_build_runs",
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
            true,
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
    Timing::start().record(&places, "demo", Lane::Mac, "my-branch", false);
    assert!(
        app_timing::stats(&store.dir.join(app_timing::DATABASE), 0)
            .unwrap()
            .is_empty()
    );
}

/// A git repo at <dir> with a `bae-shots` folder, a `.app-build` that names it
/// as the shots source, and itself as its `origin`.
fn shots_repo(dir: &std::path::Path) {
    crate::testing::repo(dir, "target\n");
    std::fs::create_dir_all(dir.join("bae-shots")).unwrap();
    std::fs::write(dir.join("bae-shots/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        dir.join(CONFIG),
        "step mac macos true\nshots mac true {select}\nshots-source bae-shots\n",
    )
    .unwrap();
    crate::testing::git(dir, &["add", "bae-shots", CONFIG]);
    crate::testing::git(dir, &["commit", "-q", "-m", "Add shots"]);
    crate::testing::git(
        dir,
        &["remote", "add", "origin", &dir.display().to_string()],
    );
}

fn shots_store(dir: &std::path::Path, database: &std::path::Path) -> ShotsStore {
    shots_repo(dir);
    ShotsStore {
        repo: "demo".to_string(),
        lane: Lane::Mac,
        source: "bae-shots".to_string(),
        database: database.to_path_buf(),
    }
}

const FINDINGS_RUN: &str = "echo '   Compiling x' >&2; echo '    Running `target/debug/bae-shots run`' >&2; echo 'FAIL mac.a: missing' >&2; echo 'bae-shots: 1 pair(s) fail the run: mac.a' >&2; exit 1";

#[test]
fn a_shots_run_that_printed_findings_is_kept_under_its_commit_platform_prefix_and_version() {
    let scratch = crate::testing::Scratch::new("shots-keep");
    let held = held_in(&scratch);
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let database = store.dir.join(app_timing::DATABASE);
    let top = scratch.dir.join("repo");
    let kept = shots_store(&top, &database);
    let step = fake("shots mac.a.", &["sh", "-c", FINDINGS_RUN], None);
    let result = run_steps(&top, &held, &[step], &mut Vec::new(), Some(&kept), false);
    assert!(matches!(result, Err(Fail::Exit(1))));
    let commit = resolve(&top, "HEAD").unwrap();
    let key = shot_findings::Key {
        repo: "demo".to_string(),
        version: shot_findings::version_at(&top, &commit, "bae-shots").unwrap(),
        commit,
        lane: Lane::Mac,
        prefix: "mac.a.".to_string(),
    };
    let stored = shot_findings::lookup(&database, &key)
        .unwrap()
        .expect("stored");
    assert_eq!(stored.exit_code, 1);
    assert_eq!(
        stored.findings, "FAIL mac.a: missing\nbae-shots: 1 pair(s) fail the run: mac.a\n",
        "the text after cargo's Running line"
    );
    let other = shot_findings::Key {
        version: "0".repeat(40),
        ..key
    };
    assert!(shot_findings::lookup(&database, &other).unwrap().is_none());
}

#[test]
fn a_shots_run_that_died_on_an_error_keeps_nothing() {
    let scratch = crate::testing::Scratch::new("shots-error");
    let held = held_in(&scratch);
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let database = store.dir.join(app_timing::DATABASE);
    let top = scratch.dir.join("repo");
    let kept = shots_store(&top, &database);
    let step = fake(
        "shots mac.a.",
        &[
            "sh",
            "-c",
            "echo 'bae-shots: the simulator did not start' >&2; exit 1",
        ],
        None,
    );
    let _ = run_steps(&top, &held, &[step], &mut Vec::new(), Some(&kept), false);
    let conn = db::open_reader(&database).unwrap().unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM shot_findings", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn app_build_with_only_a_shots_pick_prints_a_stored_run_and_takes_no_lane() {
    let scratch = crate::testing::Scratch::new("shots-serve");
    let held = held_in(&scratch);
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let database = store.dir.join(app_timing::DATABASE);
    let top = scratch.dir.join("repo");
    let kept = shots_store(&top, &database);
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
    let _ = run_steps(&top, &held, &[step], &mut Vec::new(), Some(&kept), false);
    let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    // Served from the store: no lane process starts (one would run this test
    // program as `builds`, and wait for a lock that does not exist).
    cmd_app_build(
        &places,
        30 * 1024 * 1024,
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
        panic!("no run of that prefix is stored")
    };
    assert!(
        message.contains("no run of mac.other.") && message.contains("builds app-build demo HEAD"),
        "{message}"
    );
}

const REPORT_RUN: &str = "mkdir -p shots/report/img; echo png > shots/report/img/a.png; echo stale > shots/report/img/old.png; echo \"<img src=\\\"img/a.png\\\">\" > shots/report/index.html; echo '    Running `target/debug/bae-shots run`' >&2; echo 'bae-shots: report at '\"$PWD\"'/shots/report/index.html' >&2; exit 0";

#[test]
fn a_shots_run_keeps_its_report_and_the_pictures_it_shows_under_its_key_in_the_store() {
    let scratch = crate::testing::Scratch::new("shots-report");
    let held = held_in(&scratch);
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let database = store.dir.join(app_timing::DATABASE);
    let top = scratch.dir.join("repo");
    let kept = shots_store(&top, &database);
    let step = fake("shots mac.a.", &["sh", "-c", REPORT_RUN], None);
    run_steps(&top, &held, &[step], &mut Vec::new(), Some(&kept), false).unwrap();
    let commit = resolve(&top, "HEAD").unwrap();
    let key = shot_findings::Key {
        repo: "demo".to_string(),
        version: shot_findings::version_at(&top, &commit, "bae-shots").unwrap(),
        commit,
        lane: Lane::Mac,
        prefix: "mac.a.".to_string(),
    };
    let folder = shot_findings::report_folder(&database, &key);
    assert!(folder.starts_with(store.dir.join("shots")), "{folder:?}");
    assert_eq!(
        std::fs::read_to_string(folder.join("img/a.png")).unwrap(),
        "png\n"
    );
    assert!(folder.join("index.html").exists());
    assert!(
        !folder.join("img/old.png").exists(),
        "only the pictures the page shows are kept"
    );
    // The next run in the checkout rewrites the checkout's folder; the kept
    // copy stays.
    std::fs::remove_dir_all(top.join("shots")).unwrap();
    assert!(folder.join("img/a.png").exists());
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
