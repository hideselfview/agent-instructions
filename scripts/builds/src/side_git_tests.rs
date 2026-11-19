use super::*;
use crate::testing::{Scratch, git, repo};

/// Commits <file> holding <text> in <top> with <subject>.
fn commit(top: &Path, file: &str, text: &str, subject: &str) {
    std::fs::write(top.join(file), text).unwrap();
    git(top, &["add", file]);
    git(top, &["commit", "-q", "-m", subject]);
}

fn subjects(found: &[Commit]) -> Vec<&str> {
    found.iter().map(|c| c.subject.as_str()).collect()
}

/// A commit waits to land until origin/main holds a copy of it: one with the
/// same patch under another subject, or one with the same author time and
/// subject under another patch. A commit with neither waits.
#[test]
fn a_commit_waits_until_origin_main_holds_a_copy_by_patch_or_by_author_and_subject() {
    let scratch = Scratch::new("side-git-copies");
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", &origin.display().to_string()],
    );
    git(
        &main,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&main, &["push", "-q", "origin", "main"]);
    git(&main, &["fetch", "-q", "origin"]);
    git(&main, &["switch", "-q", "-c", "work"]);
    commit(&main, "patch", "same\n", "Land by patch");
    commit(&main, "moved", "one\n", "Land by subject");
    commit(&main, "left", "left\n", "Leave behind");
    git(&main, &["switch", "-q", "main"]);
    // The first lands with a reworded subject, the second with another patch
    // and its author time kept.
    git(&main, &["cherry-pick", "work~2"]);
    git(
        &main,
        &["commit", "-q", "--amend", "-m", "Reworded on landing"],
    );
    git(&main, &["cherry-pick", "-n", "work~1"]);
    std::fs::write(main.join("moved"), "one, changed\n").unwrap();
    git(&main, &["add", "moved"]);
    git(&main, &["commit", "-q", "-C", "work~1"]);
    git(&main, &["push", "-q", "origin", "main"]);
    git(&main, &["fetch", "-q", "origin"]);

    let found = waiting_to_land(&main, "work").unwrap();
    assert_eq!(subjects(&found), ["Leave behind"]);
    assert!(waiting_to_land(&main, "origin/main").unwrap().is_empty());
}

#[test]
fn a_side_read_of_a_checkout_read_as_main_says_so_instead_of_vanishing() {
    let main = SideGit::Main { head: None };
    assert_eq!(
        main.side_read(),
        Err("the daemon read only HEAD here, as it does for a main checkout".to_string())
    );
    let failed = SideGit::Failed {
        error: "git failed".to_string(),
    };
    assert_eq!(failed.side_read(), Err("git failed".to_string()));
}
