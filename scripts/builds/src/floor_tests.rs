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

/// An incremental folder in <profile>, with a file in a session folder as
/// rustc lays it out, nothing written in it for <hours> (0: just written).
fn increment(profile: &std::path::Path, name: &str, hours: u64) -> std::path::PathBuf {
    std::fs::create_dir_all(profile.join(".fingerprint")).unwrap();
    let dir = profile.join("incremental").join(name);
    fill(&dir.join("s-1/query-cache.bin"), 64);
    if hours > 0 {
        age_all(&dir, hours);
    }
    dir
}

#[test]
fn under_the_floor_idle_incremental_folders_go_in_every_checkout() {
    let scratch = Scratch::new("floor-incremental");
    let tool = scratch.tool(0);
    three_checkouts(&scratch, &tool);
    let root = tool.places.root.join("demo");
    let claimed = root.join("side-1/target/debug");
    let idle = increment(&claimed, "idle-1a2b", 2);
    let fresh = increment(&claimed, "fresh-3c4d", 0);
    let held = increment(&claimed, "held-5e6f", 2);
    let main_idle = increment(&root.join("main/target/debug"), "idle-7a8b", 2);
    // A build holds the release profile's cargo lock, as cargo does for a
    // whole build.
    let locked_profile = root.join("side-1/target/release");
    let locked = increment(&locked_profile, "locked-9c0d", 2);
    let lock = locked_profile.join(".cargo-lock");
    std::fs::write(&lock, "").unwrap();
    let mut build = std::process::Command::new("lockf")
        .arg("-k")
        .arg(&lock)
        .args(["sh", "-c", "echo held; exec sleep 60"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(build.stdout.take().unwrap()), &mut said).unwrap();
    assert_eq!(said, "held\n");
    let mut sleeper = std::process::Command::new("sleep").arg("60").current_dir(held.join("s-1")).spawn().unwrap();

    let tool = scratch.tool(1 << 30);
    let roots = all_roots(&tool.places).unwrap();
    let kept = keep_floor(&tool, &roots);
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
    build.kill().unwrap();
    build.wait().unwrap();
    kept.unwrap();
    assert!(!idle.exists(), "a claimed worktree's idle incremental folder was kept");
    assert!(!main_idle.exists(), "main's idle incremental folder was kept");
    assert!(fresh.exists(), "an incremental folder written just now was deleted");
    assert!(held.exists(), "an incremental folder a process works in was deleted");
    assert!(locked.exists(), "an incremental folder whose profile a build holds was deleted");
    assert!(root.join("side-1/target/blob").exists(), "a claimed worktree's folder was deleted");
}
