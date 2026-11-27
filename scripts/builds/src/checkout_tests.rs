use super::*;
use crate::testing::{Scratch, git, repo};
use std::os::unix::fs::PermissionsExt;

/// A repo `demo` pushed to a bare origin, with `side-1` claimed.
fn demo_with_a_side(scratch: &Scratch) -> (crate::tool::Tool, PathBuf) {
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    let origin_path = origin.display().to_string();
    git(&scratch.dir, &["init", "-q", "--bare", &origin_path]);
    git(&main, &["remote", "add", "origin", &origin_path]);
    git(&main, &["push", "-q", "origin", "main"]);
    crate::worktree::cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &crate::args::Args::of(
            crate::command::Command::New,
            &[
                main.display().to_string(),
                "--for".to_string(),
                "agent".to_string(),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    let top = scratch.dir.join("demo-worktrees/side-1");
    (tool, top)
}

fn mode(path: &Path, bits: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits)).unwrap();
}

/// What reading the build roots says while <path> has no permissions, with
/// the permissions restored after.
fn roots_while_closed(tool: &crate::tool::Tool, path: &Path) -> String {
    mode(path, 0o000);
    let roots = tool.roots();
    mode(path, 0o755);
    match roots {
        Err(error) => error.text(),
        Ok(_) => panic!(
            "reading the roots did not fail while {} was closed",
            path.display()
        ),
    }
}

/// A claimed worktree whose git dir (which holds its claim) git cannot open is
/// not a gone checkout: `tool.roots()`, which `builds health` and the volume
/// floor read, fails and names the worktree. A gone root's build folders are
/// what the floor deletes.
#[test]
fn a_worktree_whose_git_dir_git_cannot_open_fails_the_read_of_the_roots() {
    let scratch = Scratch::new("checkout-unreadable-git-dir");
    let (tool, top) = demo_with_a_side(&scratch);
    let text = roots_while_closed(&tool, &scratch.dir.join("demo/.git/worktrees/side-1"));
    assert!(text.contains(&top.display().to_string()), "{text}");
}

/// A worktree folder that cannot be searched is not a gone checkout either.
#[test]
fn a_worktree_folder_that_cannot_be_searched_fails_the_read_of_the_roots() {
    let scratch = Scratch::new("checkout-unsearchable-folder");
    let (tool, top) = demo_with_a_side(&scratch);
    let text = roots_while_closed(&tool, &top);
    assert!(text.contains(&top.display().to_string()), "{text}");
}

/// A worktree inside main's folder whose git dir is gone is a gone checkout:
/// its own `.git` file decides, not main's `.git` above it.
#[test]
fn a_nested_worktree_whose_git_dir_is_gone_is_not_a_checkout() {
    let scratch = Scratch::new("checkout-nested-gone");
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let nested = main.join(".claude/worktrees/one");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        nested.join(".git"),
        format!("gitdir: {}/.git/worktrees/one\n", main.display()),
    )
    .unwrap();
    assert!(Checkout::at(&nested).unwrap().is_none());
}
