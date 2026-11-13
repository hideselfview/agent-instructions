use super::*;
use crate::app_config::{parse_config, run_git};
use crate::testing::{git, repo, Scratch};

#[test]
fn the_copy_replaces_the_test_app() {
    let scratch = Scratch::new("build-app-copy");
    let places = scratch.places();
    let config = parse_config("mac build b\nmac app out/out.app\nmac bundle demo.test\n").unwrap();
    let app = copy_path(&places, "demo", config.mac.as_ref().unwrap()).unwrap();
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
fn a_platform_the_config_lacks_is_refused() {
    let config = parse_config("android build ./a.sh\n").unwrap();
    assert_eq!(Platform::Android.build(&config).unwrap(), ["./a.sh".to_string()]);
    assert!(Platform::Mac.build(&config).is_err());
    assert!(Platform::parse("ios").is_err());
}

#[test]
fn a_platform_builds_in_its_lane() {
    assert_eq!(Platform::Mac.lane(), Lane::Mac);
    assert_eq!(Platform::Android.lane(), Lane::Android);
}

#[test]
fn no_ref_builds_the_origin_main_the_fetch_brought_not_a_stale_local_main() {
    let scratch = Scratch::new("build-app-default-ref");
    let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
    let (no_ref, with_ref) = (args(&["demo", "mac"]), args(&["demo", "mac", "v1"]));
    let (_, _, default) = parse_args(&no_ref).unwrap();
    let (_, _, explicit) = parse_args(&with_ref).unwrap();
    assert_eq!(explicit, "v1");

    let top = scratch.dir.join("top");
    repo(&top, "out.app 1G\n");
    std::fs::write(top.join(CONFIG), "mac build mkdir out.app\nmac app out.app\nmac bundle demo.test\n").unwrap();
    git(&top, &["add", CONFIG]);
    git(&top, &["commit", "-q", "-m", "Test app"]);
    let origin = scratch.dir.join("origin.git");
    git(&scratch.dir, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(&top, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&top, &["push", "-q", "origin", "main"]);

    // Another clone pushes a newer main; the app checkout's local main stays behind.
    let other = scratch.dir.join("other");
    git(&scratch.dir, &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]);
    git(&other, &["config", "user.email", "test@example.com"]);
    git(&other, &["config", "user.name", "test"]);
    std::fs::write(other.join("newer"), "").unwrap();
    git(&other, &["add", "newer"]);
    git(&other, &["commit", "-q", "-m", "Newer"]);
    git(&other, &["push", "-q", "origin", "main"]);
    let newer = run_git(&other, &["rev-parse", "HEAD"]).unwrap();

    let config = check_out(&top, default).unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap(), newer, "built the stale local main");
    run_command(&top, &config.mac.unwrap().build, None).unwrap();
    assert!(top.join("out.app").is_dir() && top.join("newer").exists());
}
