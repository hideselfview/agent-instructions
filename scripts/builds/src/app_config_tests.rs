use super::*;
use crate::testing::{Scratch, git, repo};

const CONFIG_TEXT: &str =
    "mac build mkdir out.app\nmac app out.app\nmac bundle demo.test\nandroid build ./a.sh --x\n";

/// A repo with a `.app-build`, and <name> for its app checkout.
fn checkout(scratch: &Scratch, name: &str) -> PathBuf {
    let top = scratch.dir.join(name);
    repo(&top, "target 1G\nout.app 1G\n");
    std::fs::write(top.join(CONFIG), CONFIG_TEXT).unwrap();
    git(&top, &["add", CONFIG]);
    git(&top, &["commit", "-q", "-m", "Test app"]);
    let origin = scratch.dir.join(format!("{name}.git"));
    git(
        &scratch.dir,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(&top, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&top, &["push", "-q", "origin", "main"]);
    top
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

#[test]
fn a_config_names_each_platforms_build() {
    assert_eq!(
        parse_config(CONFIG_TEXT).unwrap(),
        Config {
            mac: Some(MacApp {
                build: words("mkdir out.app"),
                app: "out.app".to_string(),
                bundle: "demo.test".to_string()
            }),
            android: Some(AndroidApp {
                build: words("./a.sh --x")
            }),
            steps: Vec::new(),
            pickers: Vec::new(),
            results: Vec::new(),
            rules: Vec::new(),
            needs: Vec::new(),
            sources: Vec::new(),
        }
    );
}

#[test]
fn a_platform_may_be_left_out_but_not_left_half_written() {
    let config = parse_config("android build ./a.sh\n").unwrap();
    assert_eq!(config.mac, None);
    assert!(
        parse_config("mac build mkdir out.app\nmac app out.app\n").is_err(),
        "no bundle"
    );
    assert!(parse_config("mac nope x\n").is_err(), "a field mac lacks");
    assert!(
        parse_config("ios build x\n").is_err(),
        "a platform there is none of"
    );
}

#[test]
fn the_commit_named_is_checked_out_and_built_in_the_checkout() {
    let scratch = Scratch::new("app-config-build");
    let top = checkout(&scratch, "demo");
    let commit = run_git(&top, &["rev-parse", "HEAD"]).unwrap();
    let config = check_out(&top, commit.trim()).unwrap().config;
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap(), commit);
    assert!(
        run_git(&top, &["symbolic-ref", "-q", "HEAD"]).is_err(),
        "the checkout is detached"
    );
    run_command(&top, &config.mac.unwrap().build, None).unwrap();
    assert!(top.join("out.app").is_dir());
}

#[test]
fn a_checkout_with_changes_builds_nothing() {
    let scratch = Scratch::new("app-config-dirty");
    let top = checkout(&scratch, "demo");
    std::fs::write(top.join("unsaved"), "").unwrap();
    assert!(check_out(&top, "origin/main").is_err());
}

#[test]
fn step_lines_name_a_command_each_in_the_order_written() {
    let config = parse_config(&format!(
        "{CONFIG_TEXT}step mac first ./a.sh --x\nstep mac second b\n"
    ))
    .unwrap();
    let step = |name: &str, command: &str| Step {
        name: name.to_string(),
        lane: Lane::Mac,
        command: words(command),
        limit: None,
        scenes: None,
        place: Where::Anywhere,
        opens_windows: false,
        ci_only: false,
    };
    assert_eq!(
        config.steps,
        vec![step("first", "./a.sh --x"), step("second", "b")]
    );
    assert!(
        parse_config(&format!("{CONFIG_TEXT}step mac lonely\n")).is_err(),
        "a step without a command"
    );
    assert!(
        parse_config(&format!("{CONFIG_TEXT}step mac a x\nstep mac a y\n")).is_err(),
        "a name twice"
    );
}

#[test]
fn a_limit_line_gives_its_step_a_time_and_the_others_the_default() {
    let config = parse_config("step mac slow ./a\nstep mac quick ./b\nlimit slow 90m\n").unwrap();
    assert_eq!(config.steps[0].limit(), Duration::from_secs(90 * 60));
    assert_eq!(config.steps[1].limit(), DEFAULT_STEP_LIMIT);
    assert_eq!(
        parse_config("limit s 30s\nstep mac s ./a\n").unwrap().steps[0].limit(),
        Duration::from_secs(30)
    );
    assert_eq!(show_duration(Duration::from_secs(2 * 3600)), "2h");
    assert_eq!(show_duration(Duration::from_secs(90 * 60)), "90m");
    assert_eq!(show_duration(Duration::from_secs(45)), "45s");
}

#[test]
fn a_limit_must_name_a_step_once_and_a_time() {
    assert!(
        parse_config("step mac s ./a\nlimit t 5m\n").is_err(),
        "a step the file lacks"
    );
    assert!(
        parse_config("step mac s ./a\nlimit s 5m\nlimit s 6m\n").is_err(),
        "two limits"
    );
    for bad in [
        "limit s",
        "limit s 5",
        "limit s 0m",
        "limit s fast",
        "limit s 5d",
    ] {
        assert!(
            parse_config(&format!("step mac s ./a\n{bad}\n")).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn a_step_names_its_lane_first() {
    let config = parse_config("step mac a ./a\nstep android b ./b --x\n").unwrap();
    assert_eq!(
        (config.steps[0].lane, config.steps[1].lane),
        (Lane::Mac, Lane::Android)
    );
    assert_eq!(config.steps[1].command, words("./b --x"));
    assert!(
        parse_config("step macos ./a\n").is_err(),
        "a name where the lane goes"
    );
    assert!(
        parse_config("step ios a ./a\n").is_err(),
        "a lane there is none of"
    );
    assert!(parse_config("step android\n").is_err(), "no name");
}

#[test]
fn a_test_line_gives_a_lane_the_command_that_runs_its_classes() {
    let config =
        parse_config("test mac ./t.sh --run-only {class}\ntest android ./g.sh --tests *{class}\n")
            .unwrap();
    assert_eq!(
        config.pickers,
        vec![
            Picker {
                lane: Lane::Mac,
                kind: PickKind::Class,
                program: None,
                command: words("./t.sh --run-only {class}"),
                opens_windows: false,
            },
            Picker {
                lane: Lane::Android,
                kind: PickKind::Class,
                program: None,
                command: words("./g.sh --tests *{class}"),
                opens_windows: false,
            },
        ]
    );
    assert_eq!(
        config
            .picker(Lane::Android, PickKind::Class)
            .unwrap()
            .command[2],
        "*{class}"
    );
    assert!(
        parse_config("step mac s ./a\n")
            .unwrap()
            .picker(Lane::Mac, PickKind::Class)
            .is_none()
    );
}

#[test]
fn a_test_line_needs_a_lane_a_class_placeholder_and_to_be_the_only_one_of_its_lane() {
    assert!(
        parse_config("test mac ./t.sh --run-only\n").is_err(),
        "no {{class}}"
    );
    assert!(parse_config("test mac\n").is_err(), "no command");
    assert!(
        parse_config("test ios ./t {class}\n").is_err(),
        "a lane there is none of"
    );
    assert!(
        parse_config("test mac ./a {class}\ntest mac ./b {class}\n").is_err(),
        "two for a lane"
    );
}

#[test]
fn the_word_that_holds_the_class_appears_once_for_each_class() {
    let test = Picker {
        lane: Lane::Mac,
        kind: PickKind::Class,
        program: None,
        command: words("./g.sh --tests *{class} --continue"),
        opens_windows: false,
    };
    assert_eq!(
        test.command_for(&["FooTest"]),
        words("./g.sh --tests *FooTest --continue")
    );
    assert_eq!(
        test.command_for(&["FooTest", "BarTest"]),
        words("./g.sh --tests *FooTest *BarTest --continue")
    );
}

/// A checkout on the commit before a new one that holds a file under a filter
/// whose smudge step sleeps and records its process group: switching to the new
/// commit stops in the middle of writing the tree. Returns the checkout, the
/// new commit and the file that holds the group.
fn checkout_mid_switch(scratch: &Scratch) -> (PathBuf, String, PathBuf) {
    let top = checkout(scratch, "demo");
    let group_file = scratch.dir.join("group");
    std::fs::write(top.join(".gitattributes"), "*.slow filter=slow\n").unwrap();
    git(&top, &["add", ".gitattributes"]);
    git(&top, &["commit", "-q", "-m", "Attributes"]);
    std::fs::write(top.join("a.slow"), "new\n").unwrap();
    git(&top, &["add", "a.slow"]);
    git(&top, &["commit", "-q", "-m", "Slow file"]);
    let new = run_git(&top, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    git(&top, &["checkout", "-q", "--detach", "HEAD~1"]);
    let smudge = format!("ps -o pgid= -p $$ > {}; sleep 1; cat", group_file.display());
    git(&top, &["config", "filter.slow.smudge", &smudge]);
    (top, new, group_file)
}

#[test]
fn a_switch_stopped_by_a_signal_to_its_group_still_ends_on_the_new_commit() {
    let scratch = Scratch::new("app-config-switch-signal");
    let (top, new, group_file) = checkout_mid_switch(&scratch);
    let switching = std::thread::spawn({
        let (top, new) = (top.clone(), new.clone());
        move || switch_to(&top, &new, false)
    });
    let group = loop {
        if let Ok(text) = std::fs::read_to_string(&group_file)
            && let Ok(group) = text.trim().parse::<i32>()
        {
            break group;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGUSR2] {
        unsafe { libc::killpg(group, signal) };
    }
    switching.join().unwrap().unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
    assert_eq!(
        run_git(&top, &["status", "--porcelain"]).unwrap(),
        "",
        "the tree is the new commit's whole"
    );
    assert_eq!(
        std::fs::read_to_string(top.join("a.slow")).unwrap(),
        "new\n"
    );
}

#[test]
fn a_shots_line_gives_a_lane_the_command_that_runs_a_selection_of_shots() {
    let config = parse_config(
        "test mac ./t {class}\nshots mac ./s run --select {select}\nshots android ./a --select={select}\n",
    )
    .unwrap();
    let mac = config.picker(Lane::Mac, PickKind::Shots).unwrap();
    assert_eq!(
        mac.command_for(&["mac.welcome."]),
        words("./s run --select mac.welcome.")
    );
    assert_eq!(
        config
            .picker(Lane::Android, PickKind::Shots)
            .unwrap()
            .command_for(&["android.x."]),
        words("./a --select=android.x.")
    );
    assert_eq!(
        config.picker(Lane::Mac, PickKind::Class).unwrap().command,
        words("./t {class}"),
        "a lane's test and shots lines are kept apart"
    );
}

#[test]
fn a_shots_line_needs_a_lane_a_select_placeholder_and_to_be_the_only_one_of_its_lane() {
    assert!(
        parse_config("shots mac ./s --select\n").is_err(),
        "no {{select}}"
    );
    assert!(
        parse_config("shots mac ./s {class}\n").is_err(),
        "the test line's placeholder"
    );
    assert!(parse_config("shots mac\n").is_err(), "no command");
    assert!(
        parse_config("shots ios ./s {select}\n").is_err(),
        "a lane there is none of"
    );
    assert!(
        parse_config("shots mac ./a {select}\nshots mac ./b {select}\n").is_err(),
        "two for a lane"
    );
    assert!(
        parse_config("test mac ./a {class}\nshots mac ./b {select}\n").is_ok(),
        "one of each"
    );
}

#[test]
fn the_shots_programs_lines_are_skipped() {
    let config =
        parse_config("shots-capture mac ./capture.sh\nshots-preview .row .name .cell\n").unwrap();
    assert_eq!(config, parse_config("").unwrap());
}

#[test]
fn results_lines_name_the_folders_a_lane_copies_back() {
    let config = parse_config(
        "step android a ./a\nresults android shots bae-android/app/build/test-results\nresults mac shots\nresults android reports/\n",
    )
    .unwrap();
    assert_eq!(
        config.results_of(Lane::Android),
        vec!["shots", "bae-android/app/build/test-results", "reports/"]
    );
    assert_eq!(config.results_of(Lane::Mac), vec!["shots"]);
    for refused in [
        "results android",
        "results ios shots",
        "results android /etc",
        "results android ../up",
    ] {
        assert!(parse_config(refused).is_err(), "{refused} is refused");
    }
}

#[test]
fn a_here_line_names_steps_that_stay_on_this_mac() {
    let config =
        parse_config("step mac a ./a\nstep mac b ./b\nstep mac c ./c\nhere a\nhere b a\n").unwrap();
    let places: Vec<Where> = config.steps.iter().map(|step| step.place).collect();
    assert_eq!(places, vec![Where::Here, Where::Here, Where::Anywhere]);
    assert!(parse_config("here a").is_err(), "a step it does not name");
    assert!(parse_config("step mac a ./a\nhere").is_err());
}

#[test]
fn a_ci_only_line_names_steps_that_only_ci_runs() {
    let config = parse_config(
        "step mac a ./a\nstep mac b ./b\nstep android c ./c\nci-only a\nci-only c a\n",
    )
    .unwrap();
    let ci_only: Vec<bool> = config.steps.iter().map(|step| step.ci_only).collect();
    assert_eq!(ci_only, vec![true, false, true]);
    assert!(
        parse_config("ci-only a\n").is_err(),
        "a step it does not name"
    );
    assert!(
        parse_config("step mac a ./a\nci-only\n").is_err(),
        "no step"
    );
}

#[test]
fn an_on_or_needs_line_naming_a_ci_only_step_is_refused() {
    assert!(parse_config("step mac a ./a\nci-only a\non app/ a\n").is_err());
    assert!(parse_config("step mac a ./a\nci-only a\nneeds mac a\n").is_err());
}

#[test]
fn an_opens_windows_line_marks_its_test_lines_as_opening_windows() {
    let config = parse_config(
        "sources mac:ui app/ui\ntest mac ./t {class}\ntest mac:ui ./t --ui {class}\ntest android ./g {class}\nopens-windows mac:ui android\n",
    )
    .unwrap();
    let opens = |lane, program| {
        config
            .picker_in(lane, PickKind::Class, program)
            .unwrap()
            .opens_windows
    };
    assert!(opens(Lane::Mac, Some("ui")));
    assert!(opens(Lane::Android, None));
    assert!(!opens(Lane::Mac, None));
    for refused in [
        "opens-windows",
        "test mac ./t {class}\nopens-windows mac:ui",
        "shots mac ./s {select}\nopens-windows mac",
        "test mac ./t {class}\nopens-windows ios",
    ] {
        assert!(
            parse_config(refused).is_err(),
            "parse_config accepts {refused}"
        );
    }
}

#[test]
fn a_shots_line_that_passes_windows_opens_windows() {
    let config = parse_config(
        "shots mac ./s run --windows --select {select}\nshots android ./s run --select {select}\n",
    )
    .unwrap();
    let opens = |lane| {
        config
            .picker_in(lane, PickKind::Shots, None)
            .unwrap()
            .opens_windows
    };
    assert!(opens(Lane::Mac));
    assert!(!opens(Lane::Android));
}

#[test]
fn rule_lines_name_the_steps_a_changed_path_needs() {
    let config = parse_config(
        "step mac a ./a\nstep android b ./b\non app/ a\non *.kt a b\nneeds mac a\nsources mac app lib\n",
    )
    .unwrap();
    assert_eq!(config.rules.len(), 2);
    assert!(config.rules[0].matches("app/x/y.swift") && !config.rules[0].matches("other/app/x"));
    assert!(config.rules[1].matches("any/where/Foo.kt") && !config.rules[1].matches("Foo.kts"));
    assert_eq!(config.needs, vec![(Lane::Mac, vec!["a".to_string()])]);
    assert_eq!(
        config.sources,
        vec![
            Source {
                lane: Lane::Mac,
                program: None,
                folder: "app".to_string()
            },
            Source {
                lane: Lane::Mac,
                program: None,
                folder: "lib".to_string()
            }
        ]
    );
}

#[test]
fn a_rule_naming_a_step_the_config_lacks_or_the_other_lane_is_refused() {
    assert!(parse_config("step mac a ./a\non app/ nope\n").is_err());
    assert!(parse_config("step mac a ./a\nneeds mac nope\n").is_err());
    assert!(parse_config("step android b ./b\nneeds mac b\n").is_err());
    assert!(parse_config("on app/\n").is_err());
    assert!(parse_config("sources ios app\n").is_err());
    assert!(parse_config("sources mac ../app\n").is_err());
}

#[test]
fn a_line_starting_with_a_hash_is_a_comment() {
    let config = parse_config("# why\nstep mac a ./a\n  # indented too\n").unwrap();
    assert_eq!(config.steps.len(), 1);
}

#[test]
fn a_test_line_and_its_sources_can_name_a_program_of_the_lane() {
    let config = parse_config(
        "step mac a ./a\nsources mac app\nsources mac:shots app/shots\ntest mac ./t {class}\ntest mac:shots ./s {class}\n",
    )
    .unwrap();
    assert_eq!(
        config.picker(Lane::Mac, PickKind::Class).unwrap().command[0],
        "./t"
    );
    assert_eq!(
        config
            .picker_in(Lane::Mac, PickKind::Class, Some("shots"))
            .unwrap()
            .command[0],
        "./s"
    );
    assert_eq!(config.sources[1].program.as_deref(), Some("shots"));
    // A program's sources need its test line; only tests have programs.
    assert!(parse_config("step mac a ./a\nsources mac:shots x\ntest mac ./t {class}\n").is_err());
    assert!(parse_config("step mac a ./a\nshots mac:x ./t {select}\n").is_err());
    assert!(
        parse_config("step mac a ./a\ntest mac:shots ./s {class}\ntest mac:shots ./s2 {class}\n")
            .is_err()
    );
    assert!(parse_config("step mac a ./a\ntest mac: ./s {class}\n").is_err());
}

#[test]
fn the_lane_and_program_word_reads_back_as_its_lane_and_program() {
    for (lane, program) in [(Lane::Mac, None), (Lane::Android, Some("ui".to_string()))] {
        let word = lane_and_program_word(lane, program.as_deref());
        assert_eq!(lane_and_program(&word), Some((lane, program)), "{word}");
    }
    assert_eq!(lane_and_program_word(Lane::Mac, Some("shots")), "mac:shots");
}

#[test]
fn a_run_of_the_checker_is_counted_by_the_same_command_with_scenes_for_run() {
    let run = words("builds shots run --platform mac --windows --select mac.welcome.");
    assert_eq!(
        counting(&run),
        Some(words(
            "builds shots scenes --platform mac --windows --select mac.welcome."
        ))
    );
    assert_eq!(counting(&words("./other.sh --select mac.")), None);
}

#[test]
fn a_run_has_its_base_limit_and_the_time_of_one_scene_for_each_scene() {
    let scenes = Scenes {
        count: words("builds shots scenes --select mac."),
        each: Duration::from_secs(2),
    };
    assert_eq!(
        scenes.limit_for(Duration::from_secs(60), 10).unwrap(),
        Duration::from_secs(80)
    );
    assert!(
        scenes
            .limit_for(Duration::from_secs(60), usize::MAX)
            .is_err()
    );
}

#[test]
fn the_number_a_counting_command_prints_is_the_scene_count() {
    assert_eq!(parse_scene_count("467\n").unwrap(), 467);
    assert!(parse_scene_count("many").is_err());
}
