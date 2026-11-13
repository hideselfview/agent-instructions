use super::*;
use crate::testing::{Scratch, git, repo};

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        crate::fail::Fail::Message(message) => message,
        crate::fail::Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn globs() -> Vec<String> {
    vec!["design/mocks/*.dc.html".into(), "design/system/components/*/".into()]
}

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|p| p.to_string()).collect()
}

fn set(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|p| p.to_string()).collect()
}

#[test]
fn a_board_file_is_a_place() {
    let changes = classify(&globs(), &paths(&["design/mocks/AlbMacCovers.dc.html", "src/main.rs"]));
    assert_eq!(changes.places, set(&["design/mocks/AlbMacCovers.dc.html"]));
    assert!(changes.shared.is_empty());
}

#[test]
fn a_component_folder_counts_once_however_many_files_change() {
    let changes = classify(
        &globs(),
        &paths(&[
            "design/system/components/MacRow/index.html",
            "design/system/components/MacRow/sub/deep.css",
            "design/system/components/Sym/index.html",
        ]),
    );
    assert_eq!(changes.places, set(&["design/system/components/MacRow/", "design/system/components/Sym/"]));
}

#[test]
fn files_that_are_no_place_under_the_design_directory_are_shared() {
    let changes = classify(
        &globs(),
        &paths(&[
            "design/theme.toml",
            "design/mocks/canvas.json",
            "design/mocks/storage/Main.dc.html",
            "design/system/components/README.md",
            "design/system/tokens.json",
            "README.md",
        ]),
    );
    assert!(changes.places.is_empty());
    assert_eq!(
        changes.shared,
        set(&[
            "design/theme.toml",
            "design/mocks/canvas.json",
            "design/mocks/storage/Main.dc.html",
            "design/system/components/README.md",
            "design/system/tokens.json",
        ])
    );
}

#[test]
fn a_repo_with_no_globs_has_no_places_and_no_shared_files() {
    assert!(classify(&[], &paths(&["design/mocks/A.dc.html"])).is_empty());
}

#[test]
fn star_and_question_mark_stay_within_a_segment() {
    assert!(segment_matches("*.dc.html", "A.dc.html"));
    assert!(!segment_matches("*.dc.html", "A.dc.htmlx"));
    assert!(segment_matches("a?c", "abc"));
    assert!(!segment_matches("a?c", "ac"));
    assert!(segment_matches("*", ""));
}

fn commit(dir: &Path, files: &[(&str, &str)], message: &str) -> String {
    for (path, text) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        git(dir, &["add", path]);
    }
    git(dir, &["commit", "-q", "-m", message]);
    full_sha(dir, "HEAD").unwrap()
}

#[test]
fn changes_come_from_each_commit_and_a_range_names_its_commits() {
    let scratch = Scratch::new("design-paths-git");
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    git(&dir, &["tag", "base"]);
    let a = commit(&dir, &[("design/mocks/A.dc.html", "1"), ("code.rs", "x")], "board");
    let b = commit(&dir, &[("design/system/components/MacRow/a.html", "1"), ("design/theme.toml", "t")], "component");
    let all = changes_of(&dir, &globs(), &[a.clone(), b.clone()]).unwrap();
    assert_eq!(all.places, set(&["design/mocks/A.dc.html", "design/system/components/MacRow/"]));
    assert_eq!(all.shared, set(&["design/theme.toml"]));
    assert_eq!(commits_of_range(&dir, "base..HEAD").unwrap(), [a, b]);
    assert!(message(commits_of_range(&dir, "HEAD..HEAD")).contains("holds no commits"));
    // The first commit of a repo has no parent and still lists its files.
    let root = full_sha(&dir, "base").unwrap();
    assert_eq!(commit_paths(&dir, &root).unwrap(), [".build-dirs", ".gitignore"]);
}

#[test]
fn a_name_with_odd_characters_comes_through_whole() {
    let scratch = Scratch::new("design-paths-odd");
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    let sha = commit(&dir, &[("design/mocks/Caf\u{e9} \"x\".dc.html", "1")], "odd");
    let changes = changes_of(&dir, &globs(), &[sha]).unwrap();
    assert_eq!(changes.places, set(&["design/mocks/Caf\u{e9} \"x\".dc.html"]));
}
