use super::*;
use crate::lane::Lane;
use crate::claim::cmd_release;
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
    git(&scratch.dir, &["init", "-q", "--bare", &origin.display().to_string()]);
    git(&main, &["remote", "add", "origin", &origin.display().to_string()]);
    git(&main, &["push", "-q", "origin", "main"]);
    // A root left by an older side-1 makes the next one side-2.
    std::fs::create_dir_all(tool.places.root.join("demo/side-1")).unwrap();

    cmd_new(&tool, &[main.display().to_string(), "--for".into(), "agent A".into()]).unwrap();
    let side = scratch.dir.join("demo-worktrees/side-2");
    assert!(side.join("target").is_symlink(), "new did not set the worktree up");
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    assert_eq!(Claim::read(&checkout.claim_file()).unwrap().unwrap().who, "agent A");

    let side_arg = side.display().to_string();
    assert!(refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg))).contains("is claimed by agent A since"));
    cmd_release(std::slice::from_ref(&side_arg)).unwrap();
    cmd_app_checkout(&tool, &["mac".to_string(), side_arg.clone()]).unwrap();
    assert!(refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg))).contains("is demo's mac app checkout"));
    std::fs::remove_file(tool.places.app_checkout_file("demo", Lane::Mac)).unwrap();
    std::fs::write(side.join("stray"), "x").unwrap();
    assert!(refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg))).contains("has uncommitted or untracked files"));
    std::fs::remove_file(side.join("stray")).unwrap();
    cmd_retire(&tool, std::slice::from_ref(&side_arg)).unwrap();
    assert!(!side.exists());
    assert!(!tool.places.root.join("demo/side-2").exists());
    assert!(refusal(cmd_retire(&tool, &[main.display().to_string()])).contains("only side worktrees retire"));
}

#[test]
fn a_checkout_is_the_app_checkout_of_one_lane_only() {
    let scratch = Scratch::new("app-checkout-lanes");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    git(&main, &["worktree", "add", "-q", "--detach", &side.display().to_string()]);
    let arg = side.display().to_string();
    cmd_app_checkout(&tool, &["android".to_string(), arg.clone()]).unwrap();
    assert_eq!(tool.places.app_lane_of("demo", "side-1").unwrap(), Some(Lane::Android));
    assert!(tool.places.app_checkout_file("demo", Lane::Mac).parent().unwrap().join("app-android-checkout").is_file());
    assert!(!tool.places.app_checkout_file("demo", Lane::Mac).exists());
    assert!(refusal(cmd_app_checkout(&tool, &["mac".to_string(), arg.clone()])).contains("android app checkout already"));
    assert!(refusal(cmd_app_checkout(&tool, &["ios".to_string(), arg])).contains("usage"));
    assert!(refusal(cmd_retire(&tool, &[side.display().to_string()])).contains("android app checkout"));
}
