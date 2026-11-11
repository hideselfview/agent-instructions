use super::*;
use std::time::Duration;
use crate::app_config::parse_config;

fn config() -> Config {
    parse_config("step mac one ./one\nstep android two ./two\nstep mac three ./three\nstep android four ./four\n").unwrap()
}

fn names(steps: &[Step]) -> Vec<&str> {
    steps.iter().map(|step| step.name.as_str()).collect()
}

#[test]
fn no_step_named_runs_every_step_of_the_lane_in_the_order_written() {
    assert_eq!(names(&chosen(&config(), Lane::Mac, &[]).unwrap()), vec!["one", "three"]);
    assert_eq!(names(&chosen(&config(), Lane::Android, &[]).unwrap()), vec!["two", "four"]);
}

#[test]
fn steps_named_run_in_the_order_named() {
    let named = ["three".to_string(), "one".to_string()];
    assert_eq!(names(&chosen(&config(), Lane::Mac, &named).unwrap()), vec!["three", "one"]);
}

#[test]
fn steps_named_go_to_the_lanes_they_are_in_each_in_the_order_named() {
    let named: Vec<String> = ["four", "three", "two"].map(str::to_string).to_vec();
    assert_eq!(names_in(&config(), Lane::Mac, &named), vec!["three"]);
    assert_eq!(names_in(&config(), Lane::Android, &named), vec!["four", "two"]);
}

#[test]
fn a_lane_refuses_a_step_of_the_other_lane() {
    let Err(Fail::Message(message)) = chosen(&config(), Lane::Mac, &["two".to_string()]) else { panic!("refused") };
    assert!(message.contains("android lane, not the mac lane"), "{message}");
}

#[test]
fn a_step_the_config_lacks_is_refused_with_the_ones_it_has() {
    let Err(Fail::Message(message)) = chosen(&config(), Lane::Mac, &["nope".to_string()]) else { panic!("refused") };
    assert!(message.contains("nope") && message.contains("one, two, three, four"), "{message}");
}

#[test]
fn steps_stop_at_the_first_failure() {
    let scratch = crate::testing::Scratch::new("app-build-steps");
    let step = |name: &str, command: &[&str]| Step { name: name.to_string(), lane: Lane::Mac, command: command.iter().map(|w| w.to_string()).collect(), limit: None };
    let steps = [step("a", &["touch", "a"]), step("b", &["false"]), step("c", &["touch", "c"])];
    assert!(matches!(run_steps(&scratch.dir, &steps), Err(Fail::Exit(1))));
    assert!(scratch.dir.join("a").exists());
    assert!(!scratch.dir.join("c").exists(), "the step after the failure did not run");
    assert_eq!(run_steps(&scratch.dir, &steps[..1]).unwrap(), Ran::Finished);
}

#[test]
fn a_step_past_its_limit_is_killed_with_what_it_started_and_fails_the_build() {
    let scratch = crate::testing::Scratch::new("app-build-limit");
    let slow = Step {
        name: "slow".to_string(),
        lane: Lane::Mac,
        command: ["sh", "-c", "sleep 300 & echo $! > background; wait"].map(str::to_string).to_vec(),
        limit: Some(Duration::from_millis(500)),
    };
    let after = Step { name: "after".to_string(), lane: Lane::Mac, command: vec!["touch".to_string(), "after".to_string()], limit: None };
    let started = std::time::Instant::now();
    let Err(Fail::Message(message)) = run_steps(&scratch.dir, &[slow, after]) else { panic!("fails") };
    assert!(started.elapsed() < Duration::from_secs(20), "killed at the limit, not when sleep ended");
    assert!(message.contains("step slow") && message.contains("limit"), "{message}");
    assert!(!scratch.dir.join("after").exists(), "the step after it did not run");
    let background: i32 = std::fs::read_to_string(scratch.dir.join("background")).unwrap().trim().parse().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(unsafe { libc::kill(background, 0) }, -1, "the step's background process was killed with its group");
}
