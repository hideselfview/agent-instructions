use super::*;
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
    let step = |name: &str, command: &[&str]| Step { name: name.to_string(), command: command.iter().map(|w| w.to_string()).collect() };
    let steps = [step("a", &["touch", "a"]), step("b", &["false"]), step("c", &["touch", "c"])];
    assert!(matches!(run_steps(&scratch.dir, &steps), Err(Fail::Exit(1))));
    assert!(scratch.dir.join("a").exists());
    assert!(!scratch.dir.join("c").exists(), "the step after the failure did not run");
    assert_eq!(run_steps(&scratch.dir, &steps[..1]).unwrap(), Ran::Finished);
}
