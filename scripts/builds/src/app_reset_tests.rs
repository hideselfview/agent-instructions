use super::*;
use crate::app_config::run_git;
use crate::host::tests::fake_ssh;
use crate::lane::Lane;
use crate::testing::{Scratch, git, git_out, host_builds, repo};
use std::path::PathBuf;

/// The ssh the tests of checkouts on this Mac pass: none of them reaches a
/// host.
fn no_ssh(scratch: &Scratch) -> Ssh {
    fake_ssh(&scratch.dir.join("bin"), &scratch.dir.join("remote-home"))
}

/// Registers a mac app checkout app-1 of the repo demo, in a clone whose
/// origin's main has a second commit (it changes `kept`, deletes `gone`, adds
/// `added`) and whose HEAD is the first. Returns the places, the checkout's
/// folder, the first commit and the second.
fn halfway(scratch: &Scratch) -> (Places, PathBuf, String, String) {
    let top = scratch.dir.join("app-1");
    repo(&top, "target 1G\n");
    std::fs::write(top.join("kept"), "old\n").unwrap();
    std::fs::write(top.join("gone"), "old\n").unwrap();
    git(&top, &["add", "kept", "gone"]);
    git(&top, &["commit", "-q", "-m", "First"]);
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(&top, &["remote", "add", "origin", origin.to_str().unwrap()]);
    let old = run_git(&top, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    std::fs::write(top.join("kept"), "new\n").unwrap();
    std::fs::write(top.join("added"), "new\n").unwrap();
    std::fs::remove_file(top.join("gone")).unwrap();
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "Second"]);
    git(&top, &["push", "-q", "origin", "main"]);
    let new = run_git(&top, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    // HEAD back on the first commit with the second's files in the tree: what a
    // checkout killed before it moved HEAD leaves.
    git(&top, &["checkout", "-q", "--detach", &old]);
    git(&top, &["checkout", "-q", &new, "--", "."]);
    git(
        &top,
        &["rm", "-q", "--cached", "-r", "--ignore-unmatch", "gone"],
    );
    let _ = std::fs::remove_file(top.join("gone"));
    git(&top, &["reset", "-q", &old]);
    assert!(
        !run_git(&top, &["status", "--porcelain"])
            .unwrap()
            .is_empty(),
        "the checkout starts dirty"
    );
    let places = scratch.places();
    let root = places.root.join("demo").join("app-1");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".checkout"), top.to_str().unwrap()).unwrap();
    places
        .write_app_checkouts(
            "demo",
            Lane::MAC,
            &[crate::lane::AppCheckout::by_person("app-1").unwrap()],
        )
        .unwrap();
    (places, top, old, new)
}

#[test]
fn a_checkout_left_halfway_goes_to_origins_main() {
    let scratch = Scratch::new("app-reset-halfway");
    let (places, top, _, new) = halfway(&scratch);
    let changed = Changed::default();
    cmd_reset(&places, &no_ssh(&scratch), "demo", "app-1", None, &changed).unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
    assert_eq!(run_git(&top, &["status", "--porcelain"]).unwrap(), "");
    assert_eq!(std::fs::read_to_string(top.join("kept")).unwrap(), "new\n");
    assert!(!top.join("gone").exists());
    // The checkout's build root is measured once the command ends.
    assert_eq!(
        changed.roots(),
        vec![places.root.join("demo").join("app-1")]
    );
}

#[test]
fn a_real_local_change_stops_the_reset_and_is_named() {
    let scratch = Scratch::new("app-reset-real-change");
    let (places, top, old, _) = halfway(&scratch);
    std::fs::write(top.join("kept"), "mine\n").unwrap();
    std::fs::write(top.join("notes"), "mine\n").unwrap();
    let Err(crate::fail::Fail::Message(message)) = cmd_reset(
        &places,
        &no_ssh(&scratch),
        "demo",
        "app-1",
        None,
        &Changed::default(),
    ) else {
        panic!("refused")
    };
    assert!(
        message.contains("kept") && message.contains("notes"),
        "{message}"
    );
    assert!(
        !message.contains("added"),
        "a path a commit holds is not named: {message}"
    );
    assert_eq!(
        run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(),
        old,
        "HEAD did not move"
    );
    assert_eq!(std::fs::read_to_string(top.join("kept")).unwrap(), "mine\n");
    assert_eq!(
        std::fs::read_to_string(top.join("notes")).unwrap(),
        "mine\n"
    );
}

#[test]
fn a_clean_checkout_is_put_on_origins_main_too() {
    let scratch = Scratch::new("app-reset-clean");
    let (places, top, old, new) = halfway(&scratch);
    git(&top, &["checkout", "-q", "--force", "--detach", &old]);
    cmd_reset(
        &places,
        &no_ssh(&scratch),
        "demo",
        "app-1",
        None,
        &Changed::default(),
    )
    .unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
}

#[test]
fn the_reset_waits_for_the_checkouts_lock() {
    let scratch = Scratch::new("app-reset-lock");
    let (places, top, _, new) = halfway(&scratch);
    let held = lock::take_checkout(&places, "demo", Lane::MAC, "app-1").unwrap();
    let resetting = std::thread::spawn({
        let places = places.clone();
        let ssh = no_ssh(&scratch);
        move || cmd_reset(&places, &ssh, "demo", "app-1", None, &Changed::default())
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        !resetting.is_finished(),
        "the reset waits while the lock is held"
    );
    assert_ne!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
    drop(held);
    resetting.join().unwrap().unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
}

#[test]
fn a_name_that_is_no_app_checkout_is_refused() {
    let scratch = Scratch::new("app-reset-unknown");
    let (places, _, _, _) = halfway(&scratch);
    assert!(
        cmd_reset(
            &places,
            &no_ssh(&scratch),
            "demo",
            "app-9",
            None,
            &Changed::default()
        )
        .is_err()
    );
}

#[test]
fn a_tree_of_an_older_target_resets_only_with_matching_naming_it() {
    let scratch = Scratch::new("app-reset-matching");
    let (places, top, old, new) = halfway(&scratch);
    // The switch was heading to a commit between the checkout's and today's
    // main; origin's main has moved on since.
    git(&top, &["checkout", "-q", "--force", "--detach", &old]);
    std::fs::write(top.join("kept"), "middle\n").unwrap();
    git(&top, &["commit", "-q", "-am", "Middle"]);
    let middle = run_git(&top, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    git(&top, &["checkout", "-q", "--detach", &old]);
    git(&top, &["checkout", "-q", &middle, "--", "."]);
    git(&top, &["reset", "-q", &old]);
    let Err(crate::fail::Fail::Message(message)) = cmd_reset(
        &places,
        &no_ssh(&scratch),
        "demo",
        "app-1",
        None,
        &Changed::default(),
    ) else {
        panic!("refused")
    };
    assert!(message.contains("kept"), "{message}");
    cmd_reset(
        &places,
        &no_ssh(&scratch),
        "demo",
        "app-1",
        Some(&middle),
        &Changed::default(),
    )
    .unwrap();
    assert_eq!(
        run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(),
        new,
        "it still ends on origin/main"
    );
    assert_eq!(std::fs::read_to_string(top.join("kept")).unwrap(), "new\n");
}

#[test]
fn matching_does_not_excuse_a_change_no_commit_holds() {
    let scratch = Scratch::new("app-reset-matching-real");
    let (places, top, old, _) = halfway(&scratch);
    std::fs::write(top.join("kept"), "mine\n").unwrap();
    let Err(crate::fail::Fail::Message(message)) = cmd_reset(
        &places,
        &no_ssh(&scratch),
        "demo",
        "app-1",
        Some(&old),
        &Changed::default(),
    ) else {
        panic!("refused")
    };
    assert!(message.contains("kept"), "{message}");
    assert_eq!(std::fs::read_to_string(top.join("kept")).unwrap(), "mine\n");
}

/// Host m1's mac app checkout `m1:mac-1` of the repo demo. This Mac's clone
/// `~/dev/demo` and the host's own clone share an origin whose main has two
/// commits: First, and Second, which changes `kept`, deletes `gone` and adds
/// `added`. The checkout, made by a build of First, has Second's `kept`.
struct OnHost {
    places: Places,
    ssh: Ssh,
    /// The checkout's folder on the host.
    top: PathBuf,
    /// The host's clone.
    clone: PathBuf,
    first: String,
    second: String,
}

fn on_host(scratch: &Scratch) -> OnHost {
    let places = scratch.places();
    let source = places.home.join("dev/demo");
    repo(&source, "target 1G\n");
    std::fs::write(source.join("kept"), "old\n").unwrap();
    std::fs::write(source.join("gone"), "old\n").unwrap();
    git(&source, &["add", "kept", "gone"]);
    git(&source, &["commit", "-q", "-m", "First"]);
    let first = git_out(&source, &["rev-parse", "HEAD"]);
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(
        &source,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&source, &["push", "-q", "origin", "main"]);
    let host_home = scratch.dir.join("remote-home");
    let clone = host_home.join("dev/demo");
    git(
        &scratch.dir,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    // The host's `builds`, whose setup the checkout's setup runs.
    host_builds(&host_home, "exit 0");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &host_home);
    crate::host::add(&places, &ssh, "m1", "user@m1").unwrap();
    places
        .write_app_checkouts(
            "demo",
            Lane::MAC,
            &[crate::lane::AppCheckout::by_person("m1:mac-1").unwrap()],
        )
        .unwrap();
    let lock = lock::take_checkout(&places, "demo", Lane::MAC, "m1:mac-1").unwrap();
    crate::app_remote::RemoteApp::find(
        &places,
        &ssh,
        "demo",
        Lane::MAC,
        "m1:mac-1",
        CheckoutLock::new(lock.as_fd(), None),
    )
    .unwrap()
    .expect("m1:mac-1 is on a host")
    .switch_to(&source, &first)
    .unwrap();
    drop(lock);
    std::fs::write(source.join("kept"), "new\n").unwrap();
    std::fs::write(source.join("added"), "new\n").unwrap();
    std::fs::remove_file(source.join("gone")).unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-q", "-m", "Second"]);
    git(&source, &["push", "-q", "origin", "main"]);
    let second = git_out(&source, &["rev-parse", "HEAD"]);
    let top = host_home.join("dev/demo-worktrees/mac-1");
    std::fs::write(top.join("kept"), "new\n").unwrap();
    OnHost {
        places,
        ssh,
        top,
        clone,
        first,
        second,
    }
}

#[test]
fn a_hosts_checkout_whose_changes_are_origins_main_goes_there() {
    let scratch = Scratch::new("app-reset-host");
    let host = on_host(&scratch);
    let changed = Changed::default();
    cmd_reset(&host.places, &host.ssh, "demo", "m1:mac-1", None, &changed).unwrap();
    // A checkout on a host has no build root on this Mac.
    assert_eq!(changed.roots(), Vec::<PathBuf>::new());
    assert_eq!(git_out(&host.top, &["rev-parse", "HEAD"]), host.second);
    assert_eq!(git_out(&host.top, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(host.top.join("kept")).unwrap(),
        "new\n"
    );
    assert!(!host.top.join("gone").exists());
    assert_eq!(
        git_out(&host.clone, &["for-each-ref", "refs/builds"]),
        "",
        "the reset leaves no job ref in the host's clone"
    );
}

#[test]
fn a_real_change_in_a_hosts_checkout_stops_the_reset_and_is_named() {
    let scratch = Scratch::new("app-reset-host-real-change");
    let host = on_host(&scratch);
    std::fs::write(host.top.join("gone"), "mine\n").unwrap();
    std::fs::write(host.top.join("notes"), "mine\n").unwrap();
    let Err(crate::fail::Fail::Message(message)) = cmd_reset(
        &host.places,
        &host.ssh,
        "demo",
        "m1:mac-1",
        None,
        &Changed::default(),
    ) else {
        panic!("refused")
    };
    assert!(
        message.contains("gone") && message.contains("notes"),
        "{message}"
    );
    assert!(
        !message.contains("kept"),
        "a path origin/main holds is not named: {message}"
    );
    assert_eq!(git_out(&host.top, &["rev-parse", "HEAD"]), host.first);
    assert_eq!(
        std::fs::read_to_string(host.top.join("notes")).unwrap(),
        "mine\n"
    );
    assert_eq!(
        git_out(&host.clone, &["for-each-ref", "refs/builds"]),
        "",
        "a refused reset leaves no job ref in the host's clone"
    );
}

#[test]
fn a_hosts_checkout_resets_over_a_tree_matching_names() {
    let scratch = Scratch::new("app-reset-host-matching");
    let host = on_host(&scratch);
    std::fs::write(host.top.join("kept"), "middle\n").unwrap();
    let source = host.places.home.join("dev/demo");
    git(&source, &["checkout", "-q", "--detach", &host.first]);
    std::fs::write(source.join("kept"), "middle\n").unwrap();
    git(&source, &["commit", "-q", "-am", "Middle"]);
    let middle = git_out(&source, &["rev-parse", "HEAD"]);
    assert!(
        cmd_reset(
            &host.places,
            &host.ssh,
            "demo",
            "m1:mac-1",
            None,
            &Changed::default()
        )
        .is_err()
    );
    cmd_reset(
        &host.places,
        &host.ssh,
        "demo",
        "m1:mac-1",
        Some(&middle),
        &Changed::default(),
    )
    .unwrap();
    assert_eq!(git_out(&host.top, &["rev-parse", "HEAD"]), host.second);
}

#[test]
fn a_refresh_passes_over_differing_files_and_fails_on_any_other_error() {
    let scratch = Scratch::new("app-reset-refresh");
    let (_, top, _, _) = halfway(&scratch);
    let lock = lock::take_checkout(&scratch.places(), "demo", Lane::MAC, "app-1").unwrap();
    let mut site = Site::Local(
        top.clone(),
        top.join("no.xcconfig"),
        CheckoutLock::new(lock.as_fd(), None),
    );
    let index = scratch.dir.join("index").display().to_string();
    site.git(Some(&index), &["read-tree", "HEAD"]).unwrap();
    refresh(&mut site, &index).expect("files that differ are no failure");
    std::fs::write(&index, "not an index").unwrap();
    assert!(
        refresh(&mut site, &index).is_err(),
        "a broken index is a failure"
    );
}
