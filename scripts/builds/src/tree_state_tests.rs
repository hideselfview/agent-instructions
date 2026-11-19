use super::*;
use crate::testing::{Scratch, git, git_out, repo};

#[test]
fn a_tree_state_names_head_the_branch_and_a_hash_of_the_changes_that_follows_their_content() {
    let scratch = Scratch::new("tree-state");
    let top = scratch.dir.join("demo");
    repo(&top, "target 1G\n");
    let clean = read(&top).unwrap();
    assert_eq!(clean.head, git_out(&top, &["rev-parse", "HEAD"]));
    assert_eq!(clean.branch.as_deref(), Some("main"));
    assert_eq!(clean.changes, None);
    std::fs::write(top.join("new.txt"), "one").unwrap();
    let one = read(&top).unwrap().changes.unwrap();
    std::fs::write(top.join("new.txt"), "two").unwrap();
    let two = read(&top).unwrap().changes.unwrap();
    assert_ne!(one, two);
    std::fs::write(top.join("new.txt"), "one").unwrap();
    assert_eq!(read(&top).unwrap().changes, Some(one));
    git(&top, &["checkout", "-q", "--detach"]);
    assert_eq!(read(&top).unwrap().branch, None);
}

#[test]
fn a_commit_is_on_the_runs_branch_when_the_branch_or_main_since_the_fork_holds_it() {
    let scratch = Scratch::new("tree-state-branch");
    let top = scratch.dir.join("demo");
    repo(&top, "target 1G\n");
    let old = git_out(&top, &["rev-parse", "HEAD"]);
    git(&top, &["commit", "-q", "--allow-empty", "-m", "Fork"]);
    let fork = git_out(&top, &["rev-parse", "HEAD"]);
    git(&top, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&top, &["checkout", "-q", "-b", "work"]);
    git(&top, &["commit", "-q", "--allow-empty", "-m", "Work"]);
    let work = git_out(&top, &["rev-parse", "HEAD"]);
    assert!(on_run_branch(&top, &work, Some("work"), "work").unwrap());
    assert!(on_run_branch(&top, &work, None, "work").unwrap());
    assert!(on_run_branch(&top, &fork, None, "work").unwrap());
    assert!(
        !on_run_branch(&top, &old, None, "work").unwrap(),
        "a commit before the branch started"
    );
}
