use super::*;
use crate::lists::fixtures::{db, design_drawn, design_tipped, read};
use crate::lists::{Lists, Resolution};
use crate::side_git::Commit;
use crate::testing::{Scratch, git, repo};

const BOARD: &str = "design/mocks/Alb.dc.html";

/// A repo whose <files> were committed on top of its first commit, and
/// that commit as the one commit not on origin/main.
fn branch(dir: &Path, files: &[&str]) -> Vec<Commit> {
    repo(dir, "");
    for file in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
        git(dir, &["add", file]);
    }
    git(dir, &["commit", "-q", "-m", "Draw"]);
    let full = boards::git_text(boards::git(dir).args(["rev-parse", "HEAD"]), "read HEAD")
        .unwrap()
        .trim()
        .to_string();
    vec![Commit {
        sha: full.chars().take(7).collect(),
        full,
        time: 1,
        authored: 1,
        subject: "Draw".into(),
    }]
}

/// The standing of <top>, whose commits waiting to land are <ahead>, as a
/// fresh dashboard reads it.
fn standing_of(lists: &Lists, top: &Path, ahead: &[Commit]) -> Result<Option<ItemWait>> {
    let git = SideRead {
        head: ahead.first().cloned(),
        ahead: ahead.to_vec(),
        main: "0".repeat(40),
        uncommitted: 0,
        last_write: None,
    };
    standing(&DesignReads::default(), lists, "bae", top, &git)
}

/// Lists with one design change drawn at <ahead>'s last commit in
/// <worktree>, holding <places>.
fn lists_with(worktree: &Path, ahead: &[Commit], places: &[&str]) -> (Lists, crate::lists::Id) {
    let db = db();
    let id = design_drawn(&db, &worktree.display().to_string(), &[], None);
    design_tipped(&db, id, &ahead[0].full);
    for place in places {
        crate::lists::fixtures::design_place(&db, id, place, Resolution::Matches("checked".into()));
    }
    let mut lists = read(&db);
    lists
        .design_places
        .insert("bae".into(), vec!["design/mocks/*.dc.html".into()]);
    (lists, id)
}

#[test]
fn a_branch_whose_change_waits_on_review_names_the_change() {
    let scratch = Scratch::new("design-cards-review");
    let top = scratch.dir.join("side-2");
    let ahead = branch(&top, &[BOARD]);
    let (lists, id) = lists_with(&top, &ahead, &[BOARD]);
    let title = lists.design(id).unwrap().title.clone();
    let wait = standing_of(&lists, &top, &ahead).unwrap();
    assert_eq!(wait, Some(ItemWait::Review { id: id.0, title }));
}

#[test]
fn a_design_branch_no_recorded_change_covers_waits_on_the_design_session() {
    let scratch = Scratch::new("design-cards-unrecorded");
    let top = scratch.dir.join("side-2");
    let ahead = branch(&top, &[BOARD, "design/mocks/Other.dc.html"]);
    let (lists, _) = lists_with(&top, &ahead, &[BOARD]);
    let wait = standing_of(&lists, &top, &ahead).unwrap();
    assert_eq!(wait, Some(ItemWait::Unrecorded));
}

#[test]
fn a_branch_with_no_design_files_has_no_design_standing() {
    let scratch = Scratch::new("design-cards-none");
    let top = scratch.dir.join("side-2");
    let ahead = branch(&top, &["src/main.rs"]);
    let (lists, _) = lists_with(&top, &ahead, &[BOARD]);
    assert_eq!(standing_of(&lists, &top, &ahead).unwrap(), None);
}

/// A change whose last commit git can't read is an error the card shows,
/// never a card with no design standing.
#[test]
fn a_change_whose_last_commit_git_cannot_read_is_an_error() {
    let scratch = Scratch::new("design-cards-unreadable");
    let top = scratch.dir.join("side-2");
    let ahead = branch(&top, &[BOARD]);
    let mut gone = ahead.clone();
    gone[0].full = "0".repeat(40);
    let (lists, _) = lists_with(&top, &gone, &[BOARD]);
    let fault = standing_of(&lists, &top, &ahead).unwrap_err().text();
    assert!(fault.contains(&"0".repeat(40)), "{fault}");
}
