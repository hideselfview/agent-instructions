use super::*;
use crate::lists::fixtures::{NOON, db, item, question, read, stamp};
use crate::lists::record::close;
use crate::lists::{How, Wait};
use crate::testing::Scratch;
use std::path::PathBuf;

/// A worktree of a repo whose origin/main is the repo's first commit, with
/// one commit of its own on top of it.
fn worktree_with_a_commit(scratch: &Scratch) -> PathBuf {
    let dir = scratch.dir.canonicalize().unwrap();
    let main = dir.join("bae");
    crate::testing::repo(&main, "target\n");
    crate::testing::git(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let top = dir.join("bae-worktrees/side-4");
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "work",
            &top.display().to_string(),
        ],
    );
    std::fs::write(top.join("file"), "x").unwrap();
    crate::testing::git(&top, &["add", "file"]);
    crate::testing::git(&top, &["commit", "-q", "-m", "Finish the work"]);
    top
}

/// Item 2 waits on a question and keeps <top>; the question is closed when
/// <answered>.
fn loaded(top: &std::path::Path, answered: bool) -> Vec<Loaded> {
    let db = db();
    let asked = question(&db);
    let kept_item = item(&db, vec![Wait::Question(asked)]);
    keep(
        &db,
        kept_item,
        &[KeptWork {
            worktree: top.display().to_string(),
            branch: "work".into(),
            since: stamp(NOON),
        }],
    )
    .unwrap();
    if answered {
        close(&db, asked, How::Done("shown".into()), &stamp(NOON + 60)).unwrap();
    }
    vec![Loaded {
        project: "demo".into(),
        lists: Ok(read(&db)),
    }]
}

#[test]
fn an_item_whose_last_wait_is_met_is_listed_with_its_commits_and_what_to_do() {
    let scratch = Scratch::new("kept-met");
    let top = worktree_with_a_commit(&scratch);
    let found = findings(&Ok(loaded(&top, true))).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    let text = &found[0].text;
    assert!(
        text.contains("item 2 \"Do the work numbered 2\" keeps")
            && text.contains(&format!("{} on work", top.display()))
            && text.contains("1 commit that origin/main lacks")
            && text.contains("Finish the work")
            && text.contains("question #1 done: shown: Which way for number 1")
            && text.contains("land its commits, or resume its agent with"),
        "{text}"
    );
    assert_eq!(
        found[0].command.as_deref(),
        Some(
            format!(
                "builds item take 2 --worktree {} --branch work --brief BRIEF",
                top.display()
            )
            .as_str()
        )
    );
}

#[test]
fn an_item_with_an_open_wait_or_with_no_waits_is_not_listed() {
    let scratch = Scratch::new("kept-open");
    let top = worktree_with_a_commit(&scratch);
    assert_eq!(findings(&Ok(loaded(&top, false))).unwrap(), Vec::new());
}

#[test]
fn the_waits_of_a_kept_item_say_each_open_record_or_each_that_met_them() {
    let db = db();
    let asked = question(&db);
    let blocker = item(&db, vec![]);
    let kept_item = item(
        &db,
        vec![
            Wait::Question(asked),
            Wait::Item(blocker),
            Wait::condition("the person is back", crate::lists::Ender::Coordinator),
        ],
    );
    let none = item(&db, vec![]);
    let lists = read(&db);
    assert_eq!(
        kept_waits(&lists, lists.item(none).unwrap()).unwrap(),
        KeptWaits::None
    );
    assert_eq!(
        kept_waits(&lists, lists.item(kept_item).unwrap()).unwrap(),
        KeptWaits::Open
    );
}
