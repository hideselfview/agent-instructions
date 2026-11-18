use super::*;
use crate::testing::{Scratch, commit, design_system, git, repo, tokens_json};
use std::path::PathBuf;

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().text()
}

/// Where a test keeps the design systems it generates for commits.
fn cache(scratch: &Scratch) -> PathBuf {
    scratch.dir.join("design-systems")
}

/// A worktree with a design system whose bundles hold `.bundle { color:
/// red; }` and `BUNDLE_MARK`, and a board. Nothing is committed.
fn tree(scratch: &Scratch) -> PathBuf {
    let dir = scratch.dir.join("wt");
    repo(&dir);
    design_system(
        &dir,
        "1",
        ".bundle { color: red; }",
        "window.Bae = { BUNDLE_MARK: 1 };",
    );
    let mocks = dir.join("design/mocks");
    std::fs::create_dir_all(&mocks).unwrap();
    std::fs::write(mocks.join("A.dc.html"), "<x-dc><p>A</p></x-dc>").unwrap();
    dir
}

/// The design systems kept in <cache>, one folder each.
fn kept(cache: &Path) -> usize {
    std::fs::read_dir(cache)
        .map(|entries| {
            entries
                .filter(|entry| {
                    !entry
                        .as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with('.')
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_file_is_read_from_a_directory_or_a_commit_and_a_missing_one_is_named() {
    let scratch = Scratch::new("source-read");
    let dir = tree(&scratch);
    let first = commit(&dir);
    std::fs::write(dir.join("design/mocks/A.dc.html"), "<x-dc>changed</x-dc>").unwrap();
    let at = Source::Commit {
        repo: &dir,
        rev: &first,
    };
    assert_eq!(
        at.read("design/mocks/A.dc.html", "the board").unwrap(),
        "<x-dc><p>A</p></x-dc>"
    );
    assert_eq!(
        Source::Dir(&dir)
            .read("design/mocks/A.dc.html", "the board")
            .unwrap(),
        "<x-dc>changed</x-dc>"
    );
    assert!(!at.has("design/mocks/B.dc.html").unwrap());
    assert_eq!(
        message(at.read("design/mocks/B.dc.html", "the board")),
        format!("the board {first}:design/mocks/B.dc.html is not there")
    );
    at.check().unwrap();
    let nowhere = Source::Commit {
        repo: &dir,
        rev: "no-such-branch",
    };
    assert!(message(nowhere.check()).contains("find the commit no-such-branch"));
}

#[test]
fn exists_in_tells_a_folder_a_file_and_a_missing_path_apart() {
    let scratch = Scratch::new("source-exists");
    let dir = tree(&scratch);
    commit(&dir);
    assert!(exists_in(&dir, "HEAD", "design/mocks").unwrap());
    assert!(exists_in(&dir, "HEAD", "design/mocks/A.dc.html").unwrap());
    assert!(!exists_in(&dir, "HEAD", "design/mocks/Gone.dc.html").unwrap());
}

#[test]
fn the_tokens_are_read_from_a_directory_or_a_commit_and_an_error_names_their_file() {
    let scratch = Scratch::new("source-tokens");
    let dir = tree(&scratch);
    let tokens = dir.join("design/system/tokens.json");
    assert!(
        Source::Dir(&dir)
            .tokens_css()
            .unwrap()
            .contains("--tokens: 1;")
    );
    let first = commit(&dir);
    std::fs::write(&tokens, tokens_json("2")).unwrap();
    let at = |rev: &str| Source::Commit { repo: &dir, rev }.tokens_css();
    assert!(at(&first).unwrap().contains("--tokens: 1;"));
    assert!(
        Source::Dir(&dir)
            .tokens_css()
            .unwrap()
            .contains("--tokens: 2;")
    );
    std::fs::write(&tokens, "not json").unwrap();
    assert!(
        message(Source::Dir(&dir).tokens_css())
            .starts_with(&format!("{}: reading the tokens", tokens.display()))
    );
    let broken = commit(&dir);
    assert!(message(at(&broken)).starts_with(&format!(
        "{broken}:design/system/tokens.json: reading the tokens"
    )));
    std::fs::remove_file(&tokens).unwrap();
    assert_eq!(
        message(Source::Dir(&dir).tokens_css()),
        format!(
            "the design system's tokens {} is not there",
            tokens.display()
        )
    );
}

#[test]
fn a_commits_design_system_is_generated_from_its_own_sources_once() {
    let scratch = Scratch::new("source-generated");
    let dir = tree(&scratch);
    let first = commit(&dir);
    let at = |rev: &str| {
        Source::Commit { repo: &dir, rev }
            .design_system(&cache(&scratch))
            .unwrap()
    };
    // The commit holds no generated file; its sources generate them.
    assert!(!exists_in(&dir, &first, "design/system/tokens.css").unwrap());
    assert!(!exists_in(&dir, &first, "design/system/components/bundle.js").unwrap());
    let [tokens, css, js] = at(&first);
    assert!(tokens.contains("--tokens: 1;") && css.contains(".bundle { color: red; }"));
    assert!(js.contains("BUNDLE_MARK"));
    assert_eq!(kept(&cache(&scratch)), 1);
    // Reading it again, or a commit that changes only a board, generates nothing new.
    at(&first);
    std::fs::write(dir.join("design/mocks/A.dc.html"), "<x-dc>A again</x-dc>").unwrap();
    at(&commit(&dir));
    assert_eq!(kept(&cache(&scratch)), 1);
    // A commit that changes the bundle's sources gets its own.
    std::fs::write(
        dir.join("design/system/components/src/bundle.js"),
        "window.Bae = { SECOND_MARK: 1 };",
    )
    .unwrap();
    let second = commit(&dir);
    assert!(at(&second)[2].contains("SECOND_MARK"));
    assert!(at(&first)[2].contains("BUNDLE_MARK"));
    assert_eq!(kept(&cache(&scratch)), 2);
}

#[test]
fn a_commit_whose_design_system_cannot_be_generated_says_why() {
    let scratch = Scratch::new("source-ungenerated");
    let dir = tree(&scratch);
    let read =
        |rev: &str| message(Source::Commit { repo: &dir, rev }.design_system(&cache(&scratch)));
    std::fs::write(dir.join("design/system/tokens.json"), "not json").unwrap();
    let broken = commit(&dir);
    let error = read(&broken);
    assert!(
        error.contains(&format!(
            "{broken}:design/system/tokens.json: reading the tokens"
        )),
        "{error}"
    );
    std::fs::write(dir.join("design/system/tokens.json"), tokens_json("1")).unwrap();
    std::fs::write(
        dir.join("design/system/build-bundle.py"),
        "raise SystemExit('no bundles')",
    )
    .unwrap();
    let error = read(&commit(&dir));
    assert!(
        error.contains("design/system/build-bundle.py failed") && error.contains("no bundles"),
        "{error}"
    );
    // A source that links outside the repo is refused and never passed to
    // build-bundle.py.
    std::os::unix::fs::symlink("/etc/hosts", dir.join("design/system/outside")).unwrap();
    let error = read(&commit(&dir));
    assert!(
        error.contains("cannot be generated") && error.contains("points outside the repo"),
        "{error}"
    );
    assert_eq!(kept(&cache(&scratch)), 0);
}

#[test]
fn a_design_system_source_linked_from_elsewhere_in_the_repo_is_exported_with_it() {
    let scratch = Scratch::new("source-linked");
    let dir = tree(&scratch);
    // The bundle's source is a link to a file at the repo's top, as bae's sheep logo is.
    std::fs::write(dir.join("linked.js"), "window.Bae = { LINKED_MARK: 1 };").unwrap();
    let source = dir.join("design/system/components/src/bundle.js");
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink("../../../../linked.js", &source).unwrap();
    git(&dir, &["add", "linked.js"]);
    let linked = commit(&dir);
    let js = |rev: &str| {
        Source::Commit { repo: &dir, rev }
            .design_system(&cache(&scratch))
            .unwrap()[2]
            .clone()
    };
    assert!(js(&linked).contains("LINKED_MARK"));
    // A change to the linked file is another design system.
    std::fs::write(dir.join("linked.js"), "window.Bae = { RELINKED_MARK: 1 };").unwrap();
    git(&dir, &["add", "linked.js"]);
    git(&dir, &["commit", "-q", "-m", "relinked"]);
    assert!(js("HEAD").contains("RELINKED_MARK"));
}

/// The scratch folder goes once its use ends, when the use fails too.
#[test]
fn the_scratch_folder_goes_when_its_use_fails() {
    let scratch = Scratch::new("source-scratch");
    let cache = scratch.dir.join("cache");
    let mut used = None;
    let result: Result<()> = in_scratch(&cache, |work| {
        std::fs::create_dir_all(work.join("inside")).unwrap();
        used = Some(work.to_path_buf());
        fail!("the use failed")
    });
    assert_eq!(message(result), "the use failed");
    assert!(!used.expect("the use ran").exists());
}
