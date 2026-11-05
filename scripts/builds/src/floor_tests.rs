use super::*;
use crate::checkout::{Checkout, all_roots};
use crate::claim::write_claim;
use crate::testing::{Scratch, age_all, fill, git, repo};

/// main, a claimed side-1 and an unclaimed side-2, each with 2 MiB in a
/// target folder nothing wrote to for two hours.
fn three_checkouts(scratch: &Scratch, tool: &Tool) {
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    for side in ["side-1", "side-2"] {
        let dir = scratch.dir.join("demo-worktrees").join(side);
        git(&main, &["worktree", "add", "-q", "--detach", &dir.display().to_string()]);
    }
    for top in [main.clone(), scratch.dir.join("demo-worktrees/side-1"), scratch.dir.join("demo-worktrees/side-2")] {
        let checkout = Checkout::find(&top.display().to_string()).unwrap();
        crate::setup::setup(tool, &checkout).unwrap();
        let target = checkout.build_root(&tool.places).join("target");
        fill(&target.join("blob"), 2048);
        age_all(&target, 2);
    }
    let side1 = Checkout::find(&scratch.dir.join("demo-worktrees/side-1").display().to_string()).unwrap();
    write_claim(&side1, "agent A", tool.now, None, None).unwrap();
}

#[test]
fn under_the_floor_only_an_unclaimed_side_worktree_goes() {
    let scratch = Scratch::new("floor");
    // Set up while the floor is out of the way, then raise it past any disk.
    let tool = scratch.tool(0);
    three_checkouts(&scratch, &tool);
    let tool = scratch.tool(1 << 30);
    let roots = all_roots(&tool.places).unwrap();
    keep_floor(&tool, &roots).unwrap();
    let root = tool.places.root.join("demo");
    assert!(root.join("main/target/blob").exists(), "main's folder was deleted");
    assert!(root.join("side-1/target/blob").exists(), "a claimed worktree's folder was deleted");
    assert!(!root.join("side-2/target/blob").exists(), "the idle unclaimed folder was kept");
    assert!(root.join("side-2/target").is_dir(), "the deleted folder was not recreated");
}

#[test]
fn under_the_hard_floor_the_build_stops() {
    let scratch = Scratch::new("hard-floor");
    let tool = scratch.tool(0);
    three_checkouts(&scratch, &tool);
    let mut tool = scratch.tool(1 << 30);
    tool.rules.hard_floor_kib = u64::MAX;
    let roots = all_roots(&tool.places).unwrap();
    assert!(matches!(keep_floor(&tool, &roots), Err(Fail::Exit(1))));
}
