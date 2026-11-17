use super::*;
use crate::testing::{Scratch, git as run_git, repo};

fn head(dir: &Path) -> String {
    git_text(dir, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    run_git(dir, &["add", file]);
    run_git(dir, &["commit", "-q", "-m", file]);
    head(dir)
}

#[test]
fn the_paths_of_commits_leave_out_an_empty_commit_and_a_bad_rev_fails() {
    let scratch = Scratch::new("review-commits");
    let dir = scratch.dir.join("repo");
    repo(&dir, "target 1G\n");
    let base = head(&dir);
    run_git(&dir, &["checkout", "-q", "-b", "work"]);
    let a = commit(&dir, "a.txt", "a\n");
    run_git(&dir, &["commit", "-q", "--allow-empty", "-m", "empty"]);
    let empty = head(&dir);
    let b = commit(&dir, "b.txt", "b\n");
    assert_eq!(
        in_range(&dir, &base, "work").unwrap(),
        [a.clone(), empty.clone(), b.clone()]
    );
    assert_eq!(
        paths(&dir, &[a.clone(), empty.clone(), b.clone()]).unwrap(),
        ["a.txt", "b.txt"]
    );
    assert_eq!(
        paths_of_each(&dir, &[a, empty, b]).unwrap(),
        [vec!["a.txt".to_string()], vec![], vec!["b.txt".to_string()]]
    );
    assert!(paths(&dir, &["no-such-commit".to_string()]).is_err());
}

#[test]
fn exists_says_whether_the_repo_has_the_commit() {
    let scratch = Scratch::new("review-commits-exists");
    let dir = scratch.dir.join("repo");
    repo(&dir, "target 1G\n");
    let a = commit(&dir, "a.txt", "a\n");
    assert!(exists(&dir, &a).unwrap());
    assert!(!exists(&dir, &"0".repeat(40)).unwrap());
    assert!(!exists(&scratch.dir.join("no-repo"), &a).unwrap());
}
