use super::*;
use crate::review::judge::tests::{reviewed, reviewed_under};
use crate::testing::git;

fn key_in(r: &crate::review::judge::tests::Reviewed, worktree: &Path) -> String {
    landing_key(&r.demo.store.open_existing().unwrap(), "demo", worktree).unwrap()
}

#[test]
fn a_branch_with_an_open_review_keeps_its_own_key() {
    let r = reviewed("key-own", None);
    assert_eq!(key_in(&r, &r.side), "branch:work");
}

#[test]
fn commits_after_a_review_in_a_detached_worktree_belong_to_that_review() {
    let r = reviewed_under("key-detached", None, "detached:aaaaaaa");
    git(&r.side, &["checkout", "-q", "--detach"]);
    assert_eq!(
        key_in(&r, &r.side),
        "detached:aaaaaaa",
        "the detached head holds the commit the review read"
    );
    r.demo.commit(&r.side, "b.rs");
    assert_eq!(key_in(&r, &r.side), "detached:aaaaaaa");
    git(&r.side, &["switch", "-q", "-c", "named"]);
    assert_eq!(
        key_in(&r, &r.side),
        "detached:aaaaaaa",
        "a branch made later holds the same commits"
    );
}

#[test]
fn a_worktree_without_the_commits_of_a_detached_review_has_its_own_key() {
    let r = reviewed_under("key-other", None, "detached:aaaaaaa");
    let other = r.demo.add_side(2, "agent2", "other");
    assert_eq!(key_in(&r, &other), "branch:other");
}
