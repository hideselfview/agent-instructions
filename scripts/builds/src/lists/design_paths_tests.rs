use super::*;
use crate::testing::{Scratch, git, repo};

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        crate::fail::Fail::Message(message) => message,
        crate::fail::Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn globs() -> Globs {
    Globs {
        places: vec![
            "design/mocks/*.dc.html".into(),
            "design/system/components/*/".into(),
        ],
        ..Globs::default()
    }
}

/// Bae's: sources and the spec as well.
fn full_globs() -> Globs {
    Globs {
        sources: vec![
            "design/system/components/src/".into(),
            "design/system/tokens.json".into(),
            "design/system/tokens.css".into(),
        ],
        spec: vec!["spec/*.md".into()],
        ..globs()
    }
}

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|p| p.to_string()).collect()
}

fn set(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|p| p.to_string()).collect()
}

#[test]
fn a_board_file_is_a_place() {
    let changes = classify(
        &globs(),
        &paths(&["design/mocks/AlbMacCovers.dc.html", "src/main.rs"]),
    );
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
    assert_eq!(
        changes.places,
        set(&[
            "design/system/components/MacRow/",
            "design/system/components/Sym/"
        ])
    );
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
fn a_repo_with_no_globs_has_no_design_files() {
    assert!(!classify(&Globs::default(), &paths(&["design/mocks/A.dc.html"])).is_design_change());
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
    let a = commit(
        &dir,
        &[("design/mocks/A.dc.html", "1"), ("code.rs", "x")],
        "board",
    );
    let b = commit(
        &dir,
        &[
            ("design/system/components/MacRow/a.html", "1"),
            ("design/theme.toml", "t"),
        ],
        "component",
    );
    let all = changes_of(&dir, &globs(), &[a.clone(), b.clone()]).unwrap();
    assert_eq!(
        all.places,
        set(&["design/mocks/A.dc.html", "design/system/components/MacRow/"])
    );
    assert_eq!(all.shared, set(&["design/theme.toml"]));
    assert_eq!(commits_of_range(&dir, "base..HEAD").unwrap(), [a, b]);
    assert!(message(commits_of_range(&dir, "HEAD..HEAD")).contains("holds no commits"));
    // The first commit of a repo has no parent and still lists its files.
    let root = full_sha(&dir, "base").unwrap();
    assert_eq!(
        commit_paths(&dir, &root).unwrap(),
        [".build-dirs", ".gitignore"]
    );
}

#[test]
fn a_name_with_odd_characters_comes_through_whole() {
    let scratch = Scratch::new("design-paths-odd");
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    let sha = commit(
        &dir,
        &[("design/mocks/Caf\u{e9} \"x\".dc.html", "1")],
        "odd",
    );
    let changes = changes_of(&dir, &globs(), &[sha]).unwrap();
    assert_eq!(
        changes.places,
        set(&["design/mocks/Caf\u{e9} \"x\".dc.html"])
    );
}

#[test]
fn a_source_is_listed_apart_even_where_the_component_glob_would_take_it() {
    let changes = classify(
        &full_globs(),
        &paths(&[
            "design/system/components/src/10-mac.js",
            "design/system/components/src/bundle.css",
            "design/system/tokens.json",
            "design/system/components/MacRow/preview.html",
            "design/system/README.md",
            "scripts/x.sh",
        ]),
    );
    assert_eq!(
        changes.sources,
        set(&[
            "design/system/components/src/10-mac.js",
            "design/system/components/src/bundle.css",
            "design/system/tokens.json"
        ])
    );
    assert_eq!(changes.places, set(&["design/system/components/MacRow/"]));
    assert_eq!(changes.shared, set(&["design/system/README.md"]));
    assert_eq!(changes.outside, set(&["scripts/x.sh"]));
    assert!(changes.is_design_change() && !changes.only_design());
}

#[test]
fn a_spec_file_alone_is_no_design_change_and_is_outside_the_design_files() {
    let scratch = Scratch::new("design-paths-spec-alone");
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    let sha = commit(&dir, &[("spec/bae.md", "# bae\n\ntext\n")], "spec");
    let changes = changes_of(&dir, &full_globs(), &[sha]).unwrap();
    assert!(!changes.is_design_change());
    assert!(changes.spec.is_empty());
    assert_eq!(changes.outside, set(&["spec/bae.md"]));
}

#[test]
fn the_spec_sections_a_design_change_touches_are_places_by_heading() {
    let scratch = Scratch::new("design-paths-spec");
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    let text = "# bae\n\nintro\n\n## 1. One\n\none\n\n```\n# not a heading\n```\n\n### 1.1 Sub\n\nsub\n\n## 2. Two\n\ntwo\n";
    commit(&dir, &[("spec/bae.md", text)], "spec");
    git(&dir, &["tag", "specced"]);
    // Edits one line in 1.1, one in the code fence of 1. One, deletes a line of 2. Two.
    let edited = text
        .replace("sub\n", "sub changed\n")
        .replace("# not a heading", "# still not")
        .replace("\ntwo\n", "\n");
    let sha = commit(
        &dir,
        &[("spec/bae.md", &edited), ("design/mocks/A.dc.html", "1")],
        "board and spec",
    );
    let changes = changes_of(&dir, &full_globs(), &[sha]).unwrap();
    assert_eq!(changes.places, set(&["design/mocks/A.dc.html"]));
    assert_eq!(
        changes.spec,
        set(&[
            "spec/bae.md#1. One",
            "spec/bae.md#1.1 Sub",
            "spec/bae.md#2. Two"
        ])
    );
    assert!(changes.outside.is_empty() && changes.only_design());
}

#[test]
fn a_spec_file_deleted_or_edited_above_its_first_heading_is_one_place() {
    let scratch = Scratch::new("design-paths-spec-edges");
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    commit(
        &dir,
        &[
            ("spec/a.md", "preamble\n\n# A\n\nx\n"),
            ("spec/b.md", "b\n"),
        ],
        "specs",
    );
    let edit = commit(
        &dir,
        &[
            ("spec/a.md", "preamble 2\n\n# A\n\nx\n"),
            ("design/theme.toml", "t"),
        ],
        "preamble",
    );
    let changes = changes_of(&dir, &full_globs(), &[edit]).unwrap();
    assert_eq!(changes.spec, set(&["spec/a.md#(before the first heading)"]));
    git(&dir, &["rm", "-q", "spec/b.md"]);
    let gone = commit(&dir, &[("design/theme.toml", "u")], "delete b");
    let changes = changes_of(&dir, &full_globs(), &[gone]).unwrap();
    assert_eq!(changes.spec, set(&["spec/b.md#(file deleted)"]));
}

#[test]
fn a_key_names_a_place_only_in_the_form_the_glob_gives_it() {
    let g = full_globs();
    assert!(g.names_place("design/system/components/MacRow/"));
    assert!(!g.names_place("design/system/components/MacRow"));
    assert!(!g.names_place("design/system/components/MacRow/a.html"));
    assert!(g.names_place("design/mocks/A.dc.html"));
    assert!(!g.names_place("design/mocks/A.dc.html/"));
    assert!(!g.names_place("design/other/"));
    assert_eq!(PlaceKind::of("design/mocks/A.dc.html"), PlaceKind::Board);
    assert_eq!(
        PlaceKind::of("design/system/components/MacRow/"),
        PlaceKind::Component
    );
    assert_eq!(PlaceKind::of("spec/bae.md#2. Two"), PlaceKind::Spec);
}

#[test]
fn exists_in_tells_a_folder_a_file_and_a_missing_path_apart() {
    let scratch = Scratch::new("design-paths-exists");
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    commit(
        &dir,
        &[("design/system/components/MacRow/a.html", "1")],
        "component",
    );
    assert!(exists_in(&dir, "HEAD", "design/system/components/MacRow").unwrap());
    assert!(exists_in(&dir, "HEAD", "design/system/components/MacRow/a.html").unwrap());
    assert!(!exists_in(&dir, "HEAD", "design/system/components/Gone").unwrap());
}
