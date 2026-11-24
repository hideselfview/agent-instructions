use super::*;
use crate::testing::{Scratch, git as run_git, repo};

const MISSING_SHA: &str = "ffffffffffffffffffffffffffffffffffffffff";

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    run_git(dir, &["add", file]);
    run_git(dir, &["commit", "-q", "-m", file]);
    crate::testing::git_out(dir, &["rev-parse", "HEAD"])
}

#[test]
fn commit_exists_says_whether_the_repo_has_the_commit() {
    let scratch = Scratch::new("git-exists");
    let dir = scratch.dir.join("repo");
    repo(&dir, "target 1G\n");
    let a = commit(&dir, "a.txt", "a\n");
    assert!(commit_exists(&dir, &a).unwrap());
    assert!(!commit_exists(&dir, MISSING_SHA).unwrap());
}

#[test]
fn commit_exists_reports_a_git_error_outside_a_repo() {
    let scratch = Scratch::new("git-no-repo");
    let error = commit_exists(&scratch.dir, MISSING_SHA).unwrap_err().text();
    assert!(error.contains("not a git repository"), "{error}");
}

#[test]
fn commit_exists_reports_an_unreadable_alternate_object_store() {
    let scratch = Scratch::new("git-alternate");
    let dir = scratch.dir.join("repo");
    repo(&dir, "target 1G\n");
    let missing_objects = scratch.dir.join("missing-objects");
    std::fs::write(
        dir.join(".git/objects/info/alternates"),
        format!("{}\n", missing_objects.display()),
    )
    .unwrap();
    let error = commit_exists(&dir, MISSING_SHA).unwrap_err().text();
    assert!(error.contains("missing-objects"), "{error}");
}

/// branch_exists says whether a repo has a branch, and fails when git
/// cannot tell, as in a folder that is no repo.
#[test]
fn branch_exists_tells_a_missing_branch_from_a_git_failure() {
    let scratch = Scratch::new("git-branch-exists");
    let repo_dir = scratch.dir.join("repo");
    repo(&repo_dir, "target 1G\n");
    assert!(branch_exists(&repo_dir, "main").unwrap());
    assert!(!branch_exists(&repo_dir, "nope").unwrap());
    let plain = scratch.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert!(branch_exists(&plain, "main").is_err());
}

#[test]
fn a_broken_branch_ref_is_a_git_failure() {
    let scratch = Scratch::new("git-broken-branch");
    let repo_dir = scratch.dir.join("repo");
    repo(&repo_dir, "target 1G\n");
    std::fs::write(
        repo_dir.join(".git/refs/heads/broken"),
        "not a git object\n",
    )
    .unwrap();

    let note = branch_tip(&repo_dir, "broken").unwrap_err().text();
    assert!(
        note.contains("ignoring broken ref refs/heads/broken"),
        "{note}"
    );
}

#[test]
fn resolve_revision_distinguishes_an_absent_file_from_a_git_failure() {
    let scratch = Scratch::new("git-resolve-blob");
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    assert_eq!(resolve_revision(&main, "HEAD:Cargo.lock").unwrap(), None);
    assert_eq!(
        resolve_revision(&main, "origin/main:Cargo.lock").unwrap(),
        None
    );
    std::fs::write(main.join("Cargo.lock"), "# lock\n").unwrap();
    run_git(&main, &["add", "Cargo.lock"]);
    run_git(&main, &["commit", "-q", "-m", "Lock"]);
    assert_eq!(
        resolve_revision(&main, "HEAD:Cargo.lock").unwrap(),
        Some(crate::testing::git_out(
            &main,
            &["hash-object", "Cargo.lock"]
        ))
    );

    let not_a_repo = scratch.dir.join("plain");
    std::fs::create_dir_all(&not_a_repo).unwrap();
    let fault = resolve_revision(&not_a_repo, "HEAD:Cargo.lock")
        .unwrap_err()
        .text();
    assert!(fault.contains("HEAD:Cargo.lock"), "{fault}");

    std::fs::write(main.join(".git/HEAD"), "garbage\n").unwrap();
    let broken = resolve_revision(&main, "HEAD:Cargo.lock")
        .unwrap_err()
        .text();
    assert!(broken.contains("HEAD:Cargo.lock"), "{broken}");
}
