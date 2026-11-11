use super::*;
use std::time::Duration;
use crate::app_config::parse_config;

fn config() -> Config {
    parse_config("step one ./one\nstep two ./two\nstep three ./three\n").unwrap()
}

fn names(steps: &[Step]) -> Vec<&str> {
    steps.iter().map(|step| step.name.as_str()).collect()
}

#[test]
fn no_step_named_runs_every_step_in_the_order_written() {
    assert_eq!(names(&chosen(&config(), &[]).unwrap()), vec!["one", "two", "three"]);
}

#[test]
fn steps_named_run_in_the_order_named() {
    let named = ["three".to_string(), "one".to_string()];
    assert_eq!(names(&chosen(&config(), &named).unwrap()), vec!["three", "one"]);
}

#[test]
fn a_step_the_config_lacks_is_refused_with_the_ones_it_has() {
    let Err(Fail::Message(message)) = chosen(&config(), &["nope".to_string()]) else { panic!("refused") };
    assert!(message.contains("nope") && message.contains("one, two, three"), "{message}");
}

#[test]
fn steps_stop_at_the_first_failure() {
    let scratch = crate::testing::Scratch::new("app-build-steps");
    let step = |name: &str, command: &[&str]| Step { name: name.to_string(), command: command.iter().map(|w| w.to_string()).collect(), limit: None };
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
        command: ["sh", "-c", "sleep 300 & echo $! > background; wait"].map(str::to_string).to_vec(),
        limit: Some(Duration::from_millis(500)),
    };
    let after = Step { name: "after".to_string(), command: vec!["touch".to_string(), "after".to_string()], limit: None };
    let started = std::time::Instant::now();
    let Err(Fail::Message(message)) = run_steps(&scratch.dir, &[slow, after]) else { panic!("fails") };
    assert!(started.elapsed() < Duration::from_secs(20), "killed at the limit, not when sleep ended");
    assert!(message.contains("step slow") && message.contains("limit"), "{message}");
    assert!(!scratch.dir.join("after").exists(), "the step after it did not run");
    let background: i32 = std::fs::read_to_string(scratch.dir.join("background")).unwrap().trim().parse().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(unsafe { libc::kill(background, 0) }, -1, "the step's background process was killed with its group");
}
