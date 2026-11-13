use super::*;
use crate::testing::{Scratch, git};

fn repo(scratch: &Scratch) -> std::path::PathBuf {
    let top = scratch.dir.join("repo");
    std::fs::create_dir_all(&top).unwrap();
    git(&top, &["init", "-q", "-b", "main"]);
    git(&top, &["config", "user.email", "test@example.com"]);
    git(&top, &["config", "user.name", "test"]);
    git(&top, &["commit", "-q", "--allow-empty", "-m", "Start"]);
    top
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0
}

#[test]
fn the_hook_goes_in_the_git_hooks_folder_and_runs_builds_checks() {
    let scratch = Scratch::new("hook-plain");
    let top = repo(&scratch);
    let path = install(&top).unwrap();
    assert_eq!(path, std::fs::canonicalize(top.join(".git/hooks")).unwrap().join("pre-commit"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("#!/bin/sh\n") && text.contains("exec builds checks run"), "{text}");
    assert!(is_executable(&path));
}

#[test]
fn installing_again_overwrites_the_same_file() {
    let scratch = Scratch::new("hook-twice");
    let top = repo(&scratch);
    let path = install(&top).unwrap();
    std::fs::write(&path, "old").unwrap();
    assert_eq!(install(&top).unwrap(), path);
    assert!(std::fs::read_to_string(&path).unwrap().contains("builds checks run"));
}

#[test]
fn a_worktree_shares_the_main_checkouts_hook() {
    let scratch = Scratch::new("hook-worktree");
    let top = repo(&scratch);
    let side = scratch.dir.join("side");
    git(&top, &["worktree", "add", "-q", "-b", "side", side.to_str().unwrap()]);
    assert_eq!(install(&side).unwrap(), install(&top).unwrap());
}

#[test]
fn core_hooks_path_wins_when_set() {
    let scratch = Scratch::new("hook-path");
    let top = repo(&scratch);
    let hooks = scratch.dir.join("shared-hooks");
    git(&top, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let path = install(&top).unwrap();
    assert_eq!(path, std::fs::canonicalize(&hooks).unwrap().join("pre-commit"));
    assert!(is_executable(&path));
    assert!(!top.join(".git/hooks/pre-commit").exists());
}
