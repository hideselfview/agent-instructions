use super::*;
use crate::claim::cmd_release;
use crate::lane::Lane;
use crate::testing::{Scratch, git, repo};

fn refusal(result: Result<()>) -> String {
    match result {
        Err(Fail::Message(message)) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn new_claims_a_worktree_and_retire_removes_it() {
    let scratch = Scratch::new("worktree");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", &origin.display().to_string()],
    );
    git(
        &main,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&main, &["push", "-q", "origin", "main"]);
    // A root left by an older side-1 makes the next one side-2.
    std::fs::create_dir_all(tool.places.root.join("demo/side-1")).unwrap();

    cmd_new(
        &tool,
        &[main.display().to_string(), "--for".into(), "agent A".into()],
    )
    .unwrap();
    let side = scratch.dir.join("demo-worktrees/side-2");
    assert!(
        side.join("target").is_symlink(),
        "new did not set the worktree up"
    );
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    assert_eq!(
        Claim::read(&checkout.claim_file()).unwrap().unwrap().who,
        "agent A"
    );

    let side_arg = side.display().to_string();
    assert!(
        refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg)))
            .contains("is claimed by agent A since")
    );
    cmd_release(&scratch.places(), std::slice::from_ref(&side_arg)).unwrap();
    cmd_app_checkout(&tool, &["mac".to_string(), side_arg.clone()]).unwrap();
    assert!(
        refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg)))
            .contains("one of demo's mac app checkouts")
    );
    cmd_app_checkout(
        &tool,
        &["remove".to_string(), "mac".to_string(), side_arg.clone()],
    )
    .unwrap();
    std::fs::write(side.join("stray"), "x").unwrap();
    assert!(
        refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg)))
            .contains("has uncommitted or untracked files")
    );
    std::fs::remove_file(side.join("stray")).unwrap();
    cmd_retire(&tool, std::slice::from_ref(&side_arg)).unwrap();
    assert!(!side.exists());
    assert!(!tool.places.root.join("demo/side-2").exists());
    assert!(
        refusal(cmd_retire(&tool, &[main.display().to_string()]))
            .contains("only side worktrees retire")
    );
}

#[test]
fn a_checkout_is_the_app_checkout_of_one_lane_only() {
    let scratch = Scratch::new("app-checkout-lanes");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &side.display().to_string(),
        ],
    );
    let arg = side.display().to_string();
    cmd_app_checkout(&tool, &["android".to_string(), arg.clone()]).unwrap();
    assert_eq!(
        tool.places.app_lane_of("demo", "side-1").unwrap(),
        Some(Lane::Android)
    );
    assert!(
        tool.places
            .app_checkouts_file("demo", Lane::Android)
            .is_file()
    );
    assert!(!tool.places.app_checkouts_file("demo", Lane::Mac).exists());
    assert!(
        refusal(cmd_app_checkout(&tool, &["mac".to_string(), arg.clone()]))
            .contains("android app checkout already")
    );
    assert!(refusal(cmd_app_checkout(&tool, &["ios".to_string(), arg])).contains("usage"));
    assert!(
        refusal(cmd_retire(&tool, &[side.display().to_string()])).contains("android app checkouts")
    );
}

#[test]
fn a_lane_holds_several_app_checkouts_and_one_comes_out_while_no_build_holds_it() {
    let scratch = Scratch::new("app-checkout-set");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let args = |extra: &[&str], side: &std::path::Path| -> Vec<String> {
        extra
            .iter()
            .map(|word| word.to_string())
            .chain([side.display().to_string()])
            .collect()
    };
    let sides: Vec<_> = (1..=2)
        .map(|n| scratch.dir.join(format!("demo-worktrees/side-{n}")))
        .collect();
    for side in &sides {
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &side.display().to_string(),
            ],
        );
        cmd_app_checkout(&tool, &args(&["android"], side)).unwrap();
    }
    // Recording one again changes nothing.
    cmd_app_checkout(&tool, &args(&["android"], &sides[0])).unwrap();
    assert_eq!(
        tool.places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["side-1", "side-2"]
    );
    for name in ["side-1", "side-2"] {
        assert_eq!(
            tool.places.app_lane_of("demo", name).unwrap(),
            Some(Lane::Android)
        );
    }
    assert!(
        refusal(cmd_retire(&tool, &[sides[1].display().to_string()]))
            .contains("app-checkout remove android")
    );

    let lock = tool.places.app_lock_file("demo", Lane::Android, "side-2");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    // Close-on-exec: a process another test starts meanwhile must not inherit
    // the lock and keep it past the close.
    let held = crate::queue::open_with(&lock, libc::O_CREAT | libc::O_CLOEXEC).unwrap();
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &args(&["remove", "android"], &sides[1])
        ))
        .contains("holds")
    );
    assert_eq!(
        tool.places
            .app_checkouts_of("demo", Lane::Android)
            .unwrap()
            .len(),
        2
    );
    unsafe { libc::close(held) };
    // A process another test forks meanwhile has the lock until it execs.
    while crate::queue::is_held(&lock).unwrap() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cmd_app_checkout(&tool, &args(&["remove", "android"], &sides[1])).unwrap();
    assert_eq!(
        tool.places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["side-1"]
    );
    assert!(!lock.exists(), "its lock went with it");
    assert_eq!(tool.places.app_lane_of("demo", "side-2").unwrap(), None);
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &args(&["remove", "android"], &sides[1])
        ))
        .contains("not one of")
    );
    cmd_app_checkout(&tool, &args(&["remove", "android"], &sides[0])).unwrap();
    assert!(
        !tool
            .places
            .app_checkouts_file("demo", Lane::Android)
            .exists(),
        "an empty set is no file"
    );
}
