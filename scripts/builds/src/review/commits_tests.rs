use super::*;
use crate::testing::{Scratch, git as run_git, repo};

fn head(dir: &Path) -> String {
    crate::git::text(dir, &["rev-parse", "HEAD"])
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
        crate::git::commits_in_range(&dir, &base, "work").unwrap(),
        [a.clone(), empty.clone(), b.clone()]
    );
    assert_eq!(
        paths(
            &dir,
            &[a.clone(), empty.clone(), b.clone()],
            &Unread::default()
        )
        .unwrap(),
        ["a.txt", "b.txt"]
    );
    assert_eq!(
        paths_of_each(&dir, &[a, empty, b], &Unread::default())
            .unwrap()
            .into_iter()
            .map(|commit| commit.paths)
            .collect::<Vec<_>>(),
        [vec!["a.txt".to_string()], vec![], vec!["b.txt".to_string()]]
    );
    assert!(paths(&dir, &["no-such-commit".to_string()], &Unread::default()).is_err());
}

#[test]
fn only_the_files_inside_an_unread_folder_are_left_out() {
    let scratch = Scratch::new("review-commits-unread");
    let dir = scratch.dir.join("repo");
    repo(&dir, "target 1G\n");
    for folder in ["website", "app/website", "review/cases"] {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
    }
    let files = [
        "website/page.html",
        "website.rs",
        "app/website/a.rs",
        "review/cases/old.rs",
    ];
    let shas: Vec<String> = files.map(|file| commit(&dir, file, "text\n")).to_vec();
    let unread = Unread::new(&["website".to_string()]);
    assert_eq!(
        paths(&dir, &shas, &unread).unwrap(),
        ["website.rs", "app/website/a.rs"]
    );
    assert_eq!(
        files.map(|file| unread.covers(file)),
        [true, false, false, true]
    );
}
