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
fn the_page_reads_a_side_read_a_main_head_or_a_failure_as_one_object() {
    let commit = Commit {
        sha: "abc1234".to_string(),
        full: "a".repeat(40),
        time: 1,
        authored: 1,
        subject: "s".to_string(),
    };
    let side = SideGit::Side(Ok(SideRead {
        head: Some(commit.clone()),
        ahead: vec![commit.clone()],
        main: "b".repeat(40),
        uncommitted: 2,
        last_write: Some(3),
    }));
    let json = serde_json::to_value(&side).unwrap();
    assert_eq!(json["ahead"][0]["sha"], "abc1234");
    assert_eq!(json["uncommitted"], 2);
    assert_eq!(json["last_write"], 3);
    let main = serde_json::to_value(SideGit::Main(Ok(Some(commit)))).unwrap();
    assert_eq!(main["head"]["sha"], "abc1234");
    assert!(main.get("ahead").is_none(), "{main}");
    for failed in [
        SideGit::Side(Err("git failed".to_string())),
        SideGit::Main(Err("git failed".to_string())),
    ] {
        assert_eq!(
            serde_json::to_value(failed).unwrap(),
            serde_json::json!({"error": "git failed"})
        );
    }
}

/// A fixture gives the page's git objects back as the daemon sends them.
#[test]
fn each_git_object_the_page_reads_reads_back_as_the_same_object() {
    let commit = Commit {
        sha: "abc1234".to_string(),
        full: "a".repeat(40),
        time: 1,
        authored: 1,
        subject: "s".to_string(),
    };
    for git in [
        SideGit::Side(Ok(SideRead {
            head: Some(commit.clone()),
            ahead: vec![commit.clone()],
            main: "b".repeat(40),
            uncommitted: 2,
            last_write: None,
        })),
        SideGit::Main(Ok(Some(commit))),
        SideGit::Main(Ok(None)),
        SideGit::Side(Err("git failed".to_string())),
        SideGit::Main(Err("git failed".to_string())),
    ] {
        let sent = serde_json::to_value(&git).unwrap();
        let read: SideGit = serde_json::from_value(sent.clone()).unwrap();
        assert_eq!(serde_json::to_value(read).unwrap(), sent);
    }
}

/// A change to a worktree's files changes the uncommitted count and nothing
/// else of its read, so the daemon asks git for that count alone.
#[test]
fn reading_the_files_again_changes_only_the_uncommitted_files() {
    let scratch = Scratch::new("side-git-files");
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    git(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    commit(&main, "work", "one\n", "Work");
    let before = read(&main).unwrap();
    assert_eq!(before.uncommitted, 0);
    std::fs::write(main.join("new"), "x").unwrap();
    let after = read_files(&main, before.clone()).unwrap();
    assert_eq!(after.uncommitted, 1);
    assert_eq!(
        SideRead {
            uncommitted: 1,
            last_write: after.last_write,
            ..before
        },
        after
    );
    assert_eq!(after, read(&main).unwrap());
}

/// A side read that no longer fits its type is no main checkout's head.
#[test]
fn a_malformed_side_read_fails_to_read_back() {
    let bad = serde_json::json!({ "head": null, "ahead": "none" });
    assert!(serde_json::from_value::<SideGit>(bad).is_err());
}
