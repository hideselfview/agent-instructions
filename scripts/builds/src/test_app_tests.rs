use super::*;
use crate::testing::{git, repo, Scratch};

const CONFIG_TEXT: &str = "build mkdir out.app\napp out.app\nbundle demo.test\n";

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

#[test]
fn a_config_names_the_build_the_app_and_the_bundle() {
    let config = parse(CONFIG_TEXT).unwrap();
    assert_eq!(
        config,
        Config {
            build: vec!["mkdir".to_string(), "out.app".to_string()],
            app: "out.app".to_string(),
            bundle: "demo.test".to_string(),
            steps: Vec::new(),
        }
    );
}

#[test]
fn a_config_missing_a_field_is_refused() {
    assert!(parse("build mkdir out.app\napp out.app\n").is_err());
}

#[test]
fn the_commit_named_is_checked_out_and_built_in_the_checkout() {
    let scratch = Scratch::new("test-app-build");
    let top = checkout(&scratch, "demo");
    let commit = run_git(&top, &["rev-parse", "HEAD"]).unwrap();
    let config = check_out(&top, commit.trim()).unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap(), commit);
    assert!(run_git(&top, &["symbolic-ref", "-q", "HEAD"]).is_err(), "the checkout is detached");
    build(&top, &config).unwrap();
    assert!(top.join("out.app").is_dir());
}

#[test]
fn a_checkout_with_changes_builds_nothing() {
    let scratch = Scratch::new("test-app-dirty");
    let top = checkout(&scratch, "demo");
    std::fs::write(top.join("unsaved"), "").unwrap();
    assert!(check_out(&top, "origin/main").is_err());
}

#[test]
fn the_copy_replaces_the_test_app() {
    let scratch = Scratch::new("test-app-copy");
    let places = scratch.places();
    let config = parse(CONFIG_TEXT).unwrap();
    let app = copy_path(&places, "demo", &config).unwrap();
    assert_eq!(app, places.home.join("Applications/demo-test/out.app"));
    std::fs::create_dir_all(app.join("old")).unwrap();

    let built = scratch.dir.join("built/out.app");
    std::fs::create_dir_all(built.join("new")).unwrap();
    copy_out(&built, &app).unwrap();
    assert!(app.join("old").exists(), "the test app stays until its replacement is ready");
    replace(&app).unwrap();
    assert!(app.join("new").exists());
    assert!(!app.join("old").exists());
    assert!(!with_new(&app).exists());
}

#[test]
fn step_lines_name_a_command_each_in_the_order_written() {
    let config = parse(&format!("{CONFIG_TEXT}step first ./a.sh --x\nstep second b\n")).unwrap();
    let step = |name: &str, command: &[&str]| Step { name: name.to_string(), command: command.iter().map(|w| w.to_string()).collect() };
    assert_eq!(config.steps, vec![step("first", &["./a.sh", "--x"]), step("second", &["b"])]);
    assert!(parse(&format!("{CONFIG_TEXT}step lonely\n")).is_err(), "a step without a command");
    assert!(parse(&format!("{CONFIG_TEXT}step a x\nstep a y\n")).is_err(), "a name twice");
}
