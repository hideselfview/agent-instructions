use super::*;
use crate::claim::cmd_release;
use crate::testing::{Scratch, age, age_all, git, repo};
use crate::worktree::cmd_new;

/// A repo `demo` pushed to a bare origin, and a tool on a scratch volume.
fn demo(scratch: &Scratch) -> (Tool, std::path::PathBuf) {
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(&scratch.dir, &["init", "-q", "--bare", &origin.display().to_string()]);
    git(&main, &["remote", "add", "origin", &origin.display().to_string()]);
    git(&main, &["push", "-q", "origin", "main"]);
    (tool, main)
}

fn side(scratch: &Scratch, n: usize) -> std::path::PathBuf {
    scratch.dir.join(format!("demo-worktrees/side-{n}"))
}

fn release(scratch: &Scratch, n: usize) {
    cmd_release(&[side(scratch, n).display().to_string()]).unwrap();
}

fn new(tool: &Tool, main: &Path, extra: &[&str]) {
    let mut args = vec![main.display().to_string()];
    args.extend(extra.iter().map(|arg| arg.to_string()));
    cmd_new(tool, &args).unwrap();
}

fn head(top: &Path) -> String {
    git_text(crate::checkout::git(top).args(["rev-parse", "HEAD"]), "read HEAD").unwrap().trim().to_string()
}

/// Commits a file in <top>.
fn commit(top: &Path, file: &str) {
    std::fs::write(top.join(file), file).unwrap();
    git(top, &["add", file]);
    git(top, &["commit", "-q", "-m", file]);
}

use crate::checkout::git_text;

#[test]
fn new_hands_out_a_free_worktree_before_adding_one() {
    let scratch = Scratch::new("free-reuse");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["-b", "first"]);
    release(&scratch, 1);
    // Its commit landed on origin/main under another hash, as a rebase does,
    // and origin/main has moved on since.
    commit(&side(&scratch, 1), "landed");
    let sha = head(&side(&scratch, 1));
    git(&main, &["cherry-pick", &sha]);
    commit(&main, "later");
    git(&main, &["push", "-q", "origin", "main"]);

    new(&tool, &main, &["-b", "second", "--for", "agent B"]);
    assert!(!side(&scratch, 2).exists(), "a second worktree was added while side-1 was free");
    let top = side(&scratch, 1);
    assert_eq!(head(&top), head(&main), "the reused worktree is not at origin/main");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    assert_eq!(checkout.branch().unwrap().as_deref(), Some("second"));
    assert_eq!(Claim::read(&checkout.claim_file()).unwrap().unwrap().who, "agent B");
    assert!(top.join("target").is_symlink());
}

#[test]
fn new_adds_a_worktree_when_none_is_free() {
    let scratch = Scratch::new("free-none");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["--for", "agent A"]);
    new(&tool, &main, &["--for", "agent B"]);
    assert!(side(&scratch, 2).join(".git").exists(), "the claimed side-1 was handed out again");
    let checkout = Checkout::find(&side(&scratch, 2).display().to_string()).unwrap();
    assert_eq!(Claim::read(&checkout.claim_file()).unwrap().unwrap().who, "agent B");

    // A dirty one and the app checkout are not free either.
    cmd_release(&[side(&scratch, 1).display().to_string()]).unwrap();
    cmd_release(&[side(&scratch, 2).display().to_string()]).unwrap();
    std::fs::write(side(&scratch, 1).join("stray"), "x").unwrap();
    crate::worktree::cmd_app_checkout(&tool, &["mac".to_string(), side(&scratch, 2).display().to_string()]).unwrap();
    new(&tool, &main, &[]);
    assert!(side(&scratch, 3).exists());
}

#[test]
fn a_worktree_with_an_unlanded_commit_is_kept() {
    let scratch = Scratch::new("free-unlanded");
    let (tool, main) = demo(&scratch);
    // side-1 stays claimed, so the work is in side-2, which is not exempt.
    new(&tool, &main, &["--for", "agent"]);
    new(&tool, &main, &["-b", "work", "--for", "agent"]);
    release(&scratch, 2);
    let top = side(&scratch, 2);
    commit(&top, "unlanded");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    // Nothing has touched it for days.
    age_all(&checkout.build_root(&tool.places), 48);
    for name in ["index", "HEAD", ""] {
        age(&checkout.git_dir.join(name), 48);
    }

    retire_idle(&tool, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(top.exists(), "a worktree holding an unlanded commit was retired");
    new(&tool, &main, &[]);
    assert!(side(&scratch, 3).exists(), "a worktree holding an unlanded commit was handed out");
}

#[test]
fn an_idle_free_worktree_is_retired_and_the_rest_stay() {
    let scratch = Scratch::new("free-idle");
    let (tool, main) = demo(&scratch);
    // All claimed while they are made, so each `new` adds one.
    for n in 1..=5 {
        new(&tool, &main, &["--for", "agent"]);
        assert!(side(&scratch, n).exists());
    }
    let tops: Vec<_> = (1..=5).map(|n| side(&scratch, n)).collect();
    for n in [1, 2, 3, 5] {
        release(&scratch, n);
    }
    // side-1 is the long-lived one, side-4 stays claimed, side-5 is the app
    // checkout.
    crate::worktree::cmd_app_checkout(&tool, &["mac".to_string(), tops[4].display().to_string()]).unwrap();
    for top in &tops {
        let checkout = Checkout::find(&top.display().to_string()).unwrap();
        age_all(&checkout.build_root(&tool.places), 48);
        for name in ["index", "HEAD", ""] {
            age(&checkout.git_dir.join(name), 48);
        }
    }
    // side-3 was used an hour ago.
    let three = Checkout::find(&tops[2].display().to_string()).unwrap();
    age(&three.git_dir.join("HEAD"), 1);

    retire_idle(&tool, &tool.roots().unwrap(), Run::Dry).unwrap();
    assert!(tops[1].exists(), "a dry run retired a worktree");
    retire_idle(&tool, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(!tops[1].exists(), "an idle free worktree was kept");
    assert!(!tool.places.root.join("demo/side-2").exists());
    assert!(tops[0].exists(), "side-1, the long-lived worktree, was retired");
    assert!(tops[2].exists(), "a worktree used an hour ago was retired");
    assert!(tops[3].exists(), "a claimed worktree was retired");
    assert!(tops[4].exists(), "the app checkout was retired");
    assert!(main.exists());
}

#[test]
fn two_concurrent_news_never_get_the_same_worktree() {
    let scratch = Scratch::new("free-race");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["--for", "first"]);
    release(&scratch, 1);
    let other = scratch.tool(0);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| cmd_new(&tool, &[main.display().to_string(), "--for".into(), "A".into()]));
        let b = scope.spawn(|| cmd_new(&other, &[main.display().to_string(), "--for".into(), "B".into()]));
        (a.join().unwrap(), b.join().unwrap())
    });
    a.unwrap();
    b.unwrap();
    let whos: Vec<String> = (1..=2)
        .map(|n| {
            let checkout = Checkout::find(&side(&scratch, n).display().to_string()).unwrap();
            Claim::read(&checkout.claim_file()).unwrap().unwrap().who
        })
        .collect();
    assert!(whos.contains(&"A".to_string()) && whos.contains(&"B".to_string()), "{whos:?}");
    assert!(!side(&scratch, 3).exists());
}

#[test]
fn new_without_for_still_claims() {
    let scratch = Scratch::new("free-default-claim");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &[]);
    let checkout = Checkout::find(&side(&scratch, 1).display().to_string()).unwrap();
    assert_eq!(Claim::read(&checkout.claim_file()).unwrap().unwrap().who, "builds new");
}

/// A worktree Claude Code's harness made inside the main checkout, set up the
/// way a build tool would find it: with a build root.
fn harness_worktree(tool: &Tool, main: &Path) -> std::path::PathBuf {
    let top = main.join(".claude/worktrees/agent-abc123");
    git(main, &["worktree", "add", "-q", "--detach", &top.display().to_string(), "origin/main"]);
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    crate::setup::setup(tool, &checkout).unwrap();
    // Nothing has touched it for days.
    age_all(&checkout.build_root(&tool.places), 48);
    for name in ["index", "HEAD", ""] {
        age(&checkout.git_dir.join(name), 48);
    }
    top
}

#[test]
fn a_harness_worktree_is_never_handed_out() {
    let scratch = Scratch::new("free-harness-new");
    let (tool, main) = demo(&scratch);
    let harness = harness_worktree(&tool, &main);
    new(&tool, &main, &["--for", "agent"]);
    assert!(side(&scratch, 1).join(".git").exists(), "new did not add its own side-1");
    let checkout = Checkout::find(&harness.display().to_string()).unwrap();
    assert!(Claim::read(&checkout.claim_file()).unwrap().is_none(), "a harness worktree was claimed");
    assert!(find_free(&tool, &main).unwrap().is_none(), "a harness worktree counted as free");
}

#[test]
fn a_harness_worktree_is_never_retired() {
    let scratch = Scratch::new("free-harness-retire");
    let (tool, main) = demo(&scratch);
    let harness = harness_worktree(&tool, &main);
    retire_idle(&tool, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(harness.exists(), "prune retired a harness worktree");
    let error = crate::worktree::cmd_retire(&tool, &[harness.display().to_string()]).unwrap_err();
    assert!(harness.exists(), "retire removed a harness worktree: {error:?}");
}

#[test]
fn health_does_not_list_a_harness_worktree() {
    let scratch = Scratch::new("free-harness-health");
    let (tool, main) = demo(&scratch);
    let harness = harness_worktree(&tool, &main);
    std::fs::write(harness.join("stray"), "x").unwrap();
    let health = crate::health::health(&tool).unwrap();
    assert!(
        health.worktrees.iter().all(|worktree| !worktree.path.contains("agent-abc123")),
        "health listed a harness worktree"
    );
}
