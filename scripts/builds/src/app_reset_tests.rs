use super::*;
use crate::lane::Lane;
use crate::testing::{Scratch, git, repo};

/// Registers a mac app checkout side-1 of the repo demo, in a clone whose
/// origin's main has a second commit (it changes `kept`, deletes `gone`, adds
/// `added`) and whose HEAD is the first. Returns the places, the checkout's
/// folder, the first commit and the second.
fn halfway(scratch: &Scratch) -> (Places, PathBuf, String, String) {
    let top = scratch.dir.join("side-1");
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
    let root = places.root.join("demo").join("side-1");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".checkout"), top.to_str().unwrap()).unwrap();
    places
        .write_app_checkouts("demo", Lane::Mac, &["side-1".to_string()])
        .unwrap();
    (places, top, old, new)
}

#[test]
fn a_checkout_left_halfway_goes_to_origins_main() {
    let scratch = Scratch::new("app-reset-halfway");
    let (places, top, _, new) = halfway(&scratch);
    cmd_reset(&places, "demo", "side-1", None).unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
    assert_eq!(run_git(&top, &["status", "--porcelain"]).unwrap(), "");
    assert_eq!(std::fs::read_to_string(top.join("kept")).unwrap(), "new\n");
    assert!(!top.join("gone").exists());
}

#[test]
fn a_real_local_change_stops_the_reset_and_is_named() {
    let scratch = Scratch::new("app-reset-real-change");
    let (places, top, old, _) = halfway(&scratch);
    std::fs::write(top.join("kept"), "mine\n").unwrap();
    std::fs::write(top.join("notes"), "mine\n").unwrap();
    let Err(crate::fail::Fail::Message(message)) = cmd_reset(&places, "demo", "side-1", None)
    else {
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
    cmd_reset(&places, "demo", "side-1", None).unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
}

#[test]
fn the_reset_waits_for_the_checkouts_lock() {
    let scratch = Scratch::new("app-reset-lock");
    let (places, top, _, new) = halfway(&scratch);
    let held = lock::take_checkout(&places, "demo", Lane::Mac, "side-1").unwrap();
    let resetting = std::thread::spawn({
        let places = places.clone();
        move || cmd_reset(&places, "demo", "side-1", None)
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        !resetting.is_finished(),
        "the reset waits while the lock is held"
    );
    assert_ne!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
    unsafe { libc::close(held) };
    resetting.join().unwrap().unwrap();
    assert_eq!(run_git(&top, &["rev-parse", "HEAD"]).unwrap().trim(), new);
}

#[test]
fn a_name_that_is_no_app_checkout_is_refused() {
    let scratch = Scratch::new("app-reset-unknown");
    let (places, _, _, _) = halfway(&scratch);
    assert!(cmd_reset(&places, "demo", "side-9", None).is_err());
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
    let Err(crate::fail::Fail::Message(message)) = cmd_reset(&places, "demo", "side-1", None)
    else {
        panic!("refused")
    };
    assert!(message.contains("kept"), "{message}");
    cmd_reset(&places, "demo", "side-1", Some(&middle)).unwrap();
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
    let Err(crate::fail::Fail::Message(message)) = cmd_reset(&places, "demo", "side-1", Some(&old))
    else {
        panic!("refused")
    };
    assert!(message.contains("kept"), "{message}");
    assert_eq!(std::fs::read_to_string(top.join("kept")).unwrap(), "mine\n");
}
