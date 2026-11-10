use super::*;
use crate::claim::{cmd_release, write_claim};
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
    crate::worktree::cmd_app_checkout(&tool, &[side(&scratch, 2).display().to_string()]).unwrap();
    new(&tool, &main, &[]);
    assert!(side(&scratch, 3).exists());
}

#[test]
fn a_worktree_with_an_unlanded_commit_is_kept() {
    let scratch = Scratch::new("free-unlanded");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["-b", "work"]);
    let top = side(&scratch, 1);
    commit(&top, "unlanded");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    // Nothing has touched it for days.
    let root = checkout.build_root(&tool.places);
    age_all(&root, 48);
    for name in ["index", "HEAD", ""] {
        age(&checkout.git_dir.join(name), 48);
    }

    retire_idle(&tool, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(top.exists(), "a worktree holding an unlanded commit was retired");
    new(&tool, &main, &[]);
    assert!(side(&scratch, 2).exists(), "a worktree holding an unlanded commit was handed out");
}

#[test]
fn an_idle_free_worktree_is_retired_and_the_rest_stay() {
    let scratch = Scratch::new("free-idle");
    let (tool, main) = demo(&scratch);
    for n in 1..=4 {
        new(&tool, &main, &["--for", "agent"]);
        // Each `new` would reuse a released one; keep them claimed until all exist.
        assert!(side(&scratch, n).exists());
    }
    let tops: Vec<_> = (1..=4).map(|n| side(&scratch, n)).collect();
    for top in &tops {
        cmd_release(&[top.display().to_string()]).unwrap();
    }
    // side-3 is claimed again, side-4 is the app checkout.
    let three = Checkout::find(&tops[2].display().to_string()).unwrap();
    write_claim(&three, "agent", tool.now, None, None).unwrap();
    crate::worktree::cmd_app_checkout(&tool, &[tops[3].display().to_string()]).unwrap();
    for top in &tops {
        let checkout = Checkout::find(&top.display().to_string()).unwrap();
        age_all(&checkout.build_root(&tool.places), 48);
        for name in ["index", "HEAD", ""] {
            age(&checkout.git_dir.join(name), 48);
        }
    }
    // side-2 was used an hour ago.
    let two = Checkout::find(&tops[1].display().to_string()).unwrap();
    age(&two.git_dir.join("HEAD"), 1);

    retire_idle(&tool, &tool.roots().unwrap(), Run::Dry).unwrap();
    assert!(tops[0].exists(), "a dry run retired a worktree");
    retire_idle(&tool, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(!tops[0].exists(), "an idle free worktree was kept");
    assert!(!tool.places.root.join("demo/side-1").exists());
    assert!(tops[1].exists(), "a worktree used an hour ago was retired");
    assert!(tops[2].exists(), "a claimed worktree was retired");
    assert!(tops[3].exists(), "the app checkout was retired");
    assert!(main.exists());
}
