use super::*;
use crate::lists::fixtures::{db, design_drawn, read};
use crate::lists::{Lists, Resolution};
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
    let full = crate::checkout::git_text(
        crate::checkout::git(dir).args(["rev-parse", "HEAD"]),
        "read HEAD",
    )
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

fn lists_with(worktree: &Path, places: &[&str]) -> (Lists, crate::lists::Id) {
    let db = db();
    let id = design_drawn(&db, &worktree.display().to_string(), &[], vec![]);
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
    let (lists, id) = lists_with(&top, &[BOARD]);
    let title = lists.design(id).unwrap().title.clone();
    let wait = Reads::default().standing(&lists, "bae", &top, &ahead);
    assert_eq!(wait, Some(ItemWait::Review { id: id.0, title }));
}

#[test]
fn a_design_branch_no_recorded_change_covers_waits_on_the_design_session() {
    let scratch = Scratch::new("design-cards-unrecorded");
    let top = scratch.dir.join("side-2");
    let ahead = branch(&top, &[BOARD, "design/mocks/Other.dc.html"]);
    let (lists, _) = lists_with(&top, &[BOARD]);
    let wait = Reads::default().standing(&lists, "bae", &top, &ahead);
    assert_eq!(wait, Some(ItemWait::Unrecorded));
}

#[test]
fn a_branch_with_no_design_files_has_no_design_standing() {
    let scratch = Scratch::new("design-cards-none");
    let top = scratch.dir.join("side-2");
    let ahead = branch(&top, &["src/main.rs"]);
    let (lists, _) = lists_with(&top, &[BOARD]);
    assert_eq!(Reads::default().standing(&lists, "bae", &top, &ahead), None);
}
