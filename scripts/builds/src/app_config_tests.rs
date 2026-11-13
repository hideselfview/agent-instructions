use super::*;
use crate::testing::{git, repo, Scratch};

const CONFIG_TEXT: &str = "mac build mkdir out.app\nmac app out.app\nmac bundle demo.test\nandroid build ./a.sh --x\n";

/// A repo with a `.app-build`, and <name> for its app checkout.
fn checkout(scratch: &Scratch, name: &str) -> PathBuf {
    let top = scratch.dir.join(name);
    repo(&top, "target 1G\nout.app 1G\n");
    std::fs::write(top.join(CONFIG), CONFIG_TEXT).unwrap();
    git(&top, &["add", CONFIG]);
    git(&top, &["commit", "-q", "-m", "Test app"]);
    let origin = scratch.dir.join(format!("{name}.git"));
    git(&scratch.dir, &["init", "-q", "--bare", origin.to_str().unwrap()]);
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
            mac: Some(MacApp { build: words("mkdir out.app"), app: "out.app".to_string(), bundle: "demo.test".to_string() }),
            android: Some(AndroidApp { build: words("./a.sh --x") }),
            steps: Vec::new(),
            tests: Vec::new(),
        }
    );
}

#[test]
fn a_platform_may_be_left_out_but_not_left_half_written() {
    let config = parse_config("android build ./a.sh\n").unwrap();
    assert_eq!(config.mac, None);
    assert!(parse_config("mac build mkdir out.app\nmac app out.app\n").is_err(), "no bundle");
    assert!(parse_config("mac nope x\n").is_err(), "a field mac lacks");
    assert!(parse_config("ios build x\n").is_err(), "a platform there is none of");
}

#[test]
fn the_commit_named_is_checked_out_and_built_in_the_checkout() {
    let scratch = Scratch::new("app-config-build");
    let top = checkout(&scratch, "demo");
    let commit = run_git(&top, &["rev-parse", "HEAD"]).unwrap();
    let config = check_out(&top, commit.trim()).unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap(), commit);
    assert!(run_git(&top, &["symbolic-ref", "-q", "HEAD"]).is_err(), "the checkout is detached");
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
    let config = parse_config(&format!("{CONFIG_TEXT}step mac first ./a.sh --x\nstep mac second b\n")).unwrap();
    let step = |name: &str, command: &str| Step { name: name.to_string(), lane: Lane::Mac, command: words(command), limit: None };
    assert_eq!(config.steps, vec![step("first", "./a.sh --x"), step("second", "b")]);
    assert!(parse_config(&format!("{CONFIG_TEXT}step mac lonely\n")).is_err(), "a step without a command");
    assert!(parse_config(&format!("{CONFIG_TEXT}step mac a x\nstep mac a y\n")).is_err(), "a name twice");
}

#[test]
fn a_limit_line_gives_its_step_a_time_and_the_others_the_default() {
    let config = parse_config("step mac slow ./a\nstep mac quick ./b\nlimit slow 90m\n").unwrap();
    assert_eq!(config.steps[0].limit(), Duration::from_secs(90 * 60));
    assert_eq!(config.steps[1].limit(), DEFAULT_STEP_LIMIT);
    assert_eq!(parse_config("limit s 30s\nstep mac s ./a\n").unwrap().steps[0].limit(), Duration::from_secs(30));
    assert_eq!(show_duration(Duration::from_secs(2 * 3600)), "2h");
    assert_eq!(show_duration(Duration::from_secs(90 * 60)), "90m");
    assert_eq!(show_duration(Duration::from_secs(45)), "45s");
}

#[test]
fn a_limit_must_name_a_step_once_and_a_time() {
    assert!(parse_config("step mac s ./a\nlimit t 5m\n").is_err(), "a step the file lacks");
    assert!(parse_config("step mac s ./a\nlimit s 5m\nlimit s 6m\n").is_err(), "two limits");
    for bad in ["limit s", "limit s 5", "limit s 0m", "limit s fast", "limit s 5d"] {
        assert!(parse_config(&format!("step mac s ./a\n{bad}\n")).is_err(), "{bad}");
    }
}

#[test]
fn a_step_names_its_lane_first() {
    let config = parse_config("step mac a ./a\nstep android b ./b --x\n").unwrap();
    assert_eq!((config.steps[0].lane, config.steps[1].lane), (Lane::Mac, Lane::Android));
    assert_eq!(config.steps[1].command, words("./b --x"));
    assert!(parse_config("step macos ./a\n").is_err(), "a name where the lane goes");
    assert!(parse_config("step ios a ./a\n").is_err(), "a lane there is none of");
    assert!(parse_config("step android\n").is_err(), "no name");
}

#[test]
fn a_test_line_gives_a_lane_the_command_that_runs_one_class() {
    let config = parse_config("test mac ./t.sh --run-only {class}\ntest android ./g.sh --tests *{class}\n").unwrap();
    assert_eq!(
        config.tests,
        vec![
            ClassTest { lane: Lane::Mac, command: words("./t.sh --run-only {class}") },
            ClassTest { lane: Lane::Android, command: words("./g.sh --tests *{class}") },
        ]
    );
    assert_eq!(config.class_test(Lane::Android).unwrap().command[2], "*{class}");
    assert!(parse_config("step mac s ./a\n").unwrap().class_test(Lane::Mac).is_none());
}

#[test]
fn a_test_line_needs_a_lane_a_class_placeholder_and_to_be_the_only_one_of_its_lane() {
    assert!(parse_config("test mac ./t.sh --run-only\n").is_err(), "no {{class}}");
    assert!(parse_config("test mac\n").is_err(), "no command");
    assert!(parse_config("test ios ./t {class}\n").is_err(), "a lane there is none of");
    assert!(parse_config("test mac ./a {class}\ntest mac ./b {class}\n").is_err(), "two for a lane");
}

#[test]
fn the_class_fills_each_word_it_stands_in() {
    let test = ClassTest { lane: Lane::Mac, command: words("./g.sh --tests *{class}") };
    assert_eq!(test.command_for("FooTest"), words("./g.sh --tests *FooTest"));
}
