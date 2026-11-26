use super::*;
use crate::app_config::{check_out, parse_config, run_git};
use crate::testing::{Scratch, git, repo};
use std::os::fd::AsFd;

#[test]
fn the_copy_replaces_the_test_app() {
    let scratch = Scratch::new("build-app-copy");
    let places = scratch.places();
    let config = parse_config("mac build b\nmac app out/out.app\nmac bundle demo.test\n").unwrap();
    let app = copy_path(
        &places,
        "demo",
        Lane::MAC,
        copied_app(&config, Lane::MAC).unwrap(),
    )
    .unwrap();
    assert_eq!(app, places.home.join("Applications/demo-test/out.app"));
    std::fs::create_dir_all(app.join("old")).unwrap();

    let built = scratch.dir.join("built/out.app");
    std::fs::create_dir_all(built.join("new")).unwrap();
    let lock = lock::take_checkout(&places, "demo", Lane::MAC, "side-1").unwrap();
    copy_out(&built, &app, CheckoutLock::new(lock.as_fd(), None)).unwrap();
    assert!(
        app.join("old").exists(),
        "the test app stays until its replacement is ready"
    );
    replace(&app).unwrap();
    assert!(app.join("new").exists());
    assert!(!app.join("old").exists());
    assert!(!with_new(&app).exists());
}

#[test]
fn a_platform_the_config_lacks_is_refused() {
    let config = parse_config("android build ./a.sh\n").unwrap();
    assert_eq!(
        Target::of(Lane::ANDROID).build(&config).unwrap(),
        ["./a.sh".to_string()]
    );
    assert!(Target::of(Lane::MAC).build(&config).is_err());
    assert!(Target::parse(&config, "ios").is_err());
}

#[test]
fn a_platform_builds_in_its_lane() {
    let config = parse_config("mac build b\nmac app a\nmac bundle c\nandroid build d\n").unwrap();
    assert_eq!(Target::parse(&config, "mac").unwrap().lane(), Lane::MAC);
    assert_eq!(
        Target::parse(&config, "android").unwrap().lane(),
        Lane::ANDROID
    );
}

#[test]
fn no_ref_builds_the_origin_main_the_fetch_brought_not_a_stale_local_main() {
    let scratch = Scratch::new("build-app-default-ref");
    let args = |words: &[&str]| {
        words
            .iter()
            .map(|word| word.to_string())
            .collect::<Vec<_>>()
    };
    let (no_ref, with_ref) = (args(&["demo", "mac"]), args(&["demo", "mac", "v1"]));
    let (_, _, default) = parse_args(&no_ref).unwrap();
    let (_, _, explicit) = parse_args(&with_ref).unwrap();
    assert_eq!(explicit, "v1");

    let top = scratch.dir.join("top");
    repo(&top, "out.app 1G\n");
    std::fs::write(
        top.join(CONFIG),
        "mac build mkdir out.app\nmac app out.app\nmac bundle demo.test\n",
    )
    .unwrap();
    git(&top, &["add", CONFIG]);
    git(&top, &["commit", "-q", "-m", "Test app"]);
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(&top, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&top, &["push", "-q", "origin", "main"]);

    // Another clone pushes a newer main; the app checkout's local main stays behind.
    let other = scratch.dir.join("other");
    git(
        &scratch.dir,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.email", "test@example.com"]);
    git(&other, &["config", "user.name", "test"]);
    std::fs::write(other.join("newer"), "").unwrap();
    git(&other, &["add", "newer"]);
    git(&other, &["commit", "-q", "-m", "Newer"]);
    git(&other, &["push", "-q", "origin", "main"]);
    let newer = run_git(&other, &["rev-parse", "HEAD"]).unwrap();

    let lock = lock::take_checkout(&scratch.places(), "demo", Lane::MAC, "side-1").unwrap();
    let checkout = CheckoutLock::new(lock.as_fd(), None);
    let config = check_out(&top, default, checkout).unwrap().config;
    assert_eq!(
        run_git(&top, &["rev-parse", "HEAD"]).unwrap(),
        newer,
        "built the stale local main"
    );
    run_command(
        &top,
        &config.test_app_of(Lane::MAC).unwrap().build,
        checkout,
    )
    .unwrap();
    assert!(top.join("out.app").is_dir() && top.join("newer").exists());
}

#[test]
fn the_mac_build_may_take_a_hosts_checkout_unless_it_runs_a_here_step() {
    let mac = "mac build ./run.sh --no-open\nmac app out.app\nmac bundle demo.test\nstep mac signed ./run.sh --no-open\n";
    let free = parse_config(mac).unwrap();
    assert_eq!(
        Target::of(Lane::MAC).allowed(&free).unwrap(),
        Where::Anywhere
    );

    let here = parse_config(&format!("{mac}here signed\n")).unwrap();
    assert_eq!(Target::of(Lane::MAC).allowed(&here).unwrap(), Where::Here);

    let other = parse_config(&format!("{mac}step mac other ./other.sh\nhere other\n")).unwrap();
    assert_eq!(
        Target::of(Lane::MAC).allowed(&other).unwrap(),
        Where::Anywhere
    );
    assert_eq!(
        Target::of(Lane::ANDROID).allowed(&free).unwrap(),
        Where::Here
    );
}

#[test]
fn only_a_stop_signal_gives_the_status_the_dashboard_reads_as_stopped() {
    let stopped = ended(
        Err(fail::Fail::Message("the build stopped".into())),
        Some(15),
    );
    assert!(matches!(stopped, Err(fail::Fail::Exit(143))));
    // A build command that crashed (139) or lost its host (255) is a failure.
    for code in [1, 139, 255] {
        let failed = ended(Err(fail::Fail::Exit(code)), None);
        assert!(matches!(failed, Err(fail::Fail::Exit(1))), "{code}");
    }
    assert!(ended(Ok(()), None).is_ok());
}
