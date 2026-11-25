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
    assert_eq!(at.check().unwrap(), Some(first.clone()));
    let branch = Source::Commit {
        repo: &dir,
        rev: "main",
    };
    assert_eq!(branch.check().unwrap(), Some(first.clone()));
    assert_eq!(Source::Dir(&dir).check().unwrap(), None);
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

#[test]
fn a_folders_bundles_are_built_from_its_sources_and_never_read_or_written_in_it() {
    let scratch = Scratch::new("source-folder-bundles");
    let dir = tree(&scratch);
    let components = dir.join("design/system/components");
    // The bundles on disk are stale: the sources changed since the script ran.
    std::fs::write(
        components.join("src/bundle.js"),
        "window.Bae = { FRESH_MARK: 1 };",
    )
    .unwrap();
    let [_, css, js] = Source::Dir(&dir).design_system(&cache(&scratch)).unwrap();
    assert!(js.contains("FRESH_MARK") && css.contains(".bundle { color: red; }"));
    assert!(
        std::fs::read_to_string(components.join("bundle.js"))
            .unwrap()
            .contains("BUNDLE_MARK")
    );
    // A folder with no bundles on disk builds them all the same, and keeps
    // nothing in the cache or in the folder.
    std::fs::remove_file(components.join("bundle.js")).unwrap();
    std::fs::remove_file(components.join("bundle.css")).unwrap();
    let [_, _, js] = Source::Dir(&dir).design_system(&cache(&scratch)).unwrap();
    assert!(js.contains("FRESH_MARK"));
    assert!(!components.join("bundle.js").exists());
    assert_eq!(kept(&cache(&scratch)), 0);
}

#[test]
fn a_folders_design_system_follows_its_links_inside_the_repo_and_refuses_one_outside() {
    let scratch = Scratch::new("source-folder-links");
    let dir = tree(&scratch);
    std::fs::write(dir.join("linked.js"), "window.Bae = { LINKED_MARK: 1 };").unwrap();
    let source = dir.join("design/system/components/src/bundle.js");
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink("../../../../linked.js", &source).unwrap();
    let js = Source::Dir(&dir).design_system(&cache(&scratch)).unwrap()[2].clone();
    assert!(js.contains("LINKED_MARK"));
    // A link to a path the repo lacks says what reading it found.
    std::os::unix::fs::symlink("../gone.svg", dir.join("design/system/gone")).unwrap();
    let error = message(Source::Dir(&dir).design_system(&cache(&scratch)));
    assert!(
        error.contains(&format!(
            "reading {}",
            dir.join("design/gone.svg").display()
        )) && error.contains("No such file"),
        "{error}"
    );
    std::fs::remove_file(dir.join("design/system/gone")).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", dir.join("design/system/outside")).unwrap();
    let error = message(Source::Dir(&dir).design_system(&cache(&scratch)));
    assert!(
        error.contains("cannot be generated") && error.contains("points outside the repo"),
        "{error}"
    );
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

/// A scratch folder that cannot be removed fails the call, after the use's
/// own error when the use failed too.
#[test]
fn a_scratch_folder_that_cannot_be_removed_fails_the_call() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("source-scratch-kept");
    let cache = scratch.dir.join("cache");
    let locked = std::cell::RefCell::new(Vec::new());
    let lock = |work: &Path| {
        let inner = work.join("locked");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("file"), "x").unwrap();
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o500)).unwrap();
        locked.borrow_mut().push(inner);
    };
    let kept = message(in_scratch(&cache, |work| {
        lock(work);
        Ok(())
    }));
    assert!(kept.starts_with("removing the scratch folder"), "{kept}");
    let both = message(in_scratch(&cache, |work| -> Result<()> {
        lock(work);
        fail!("the use failed")
    }));
    assert!(
        both.starts_with("the use failed; then removing the scratch folder"),
        "{both}"
    );
    for inner in locked.borrow().iter() {
        std::fs::set_permissions(inner, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn a_board_draws_with_the_sf_mono_rules_and_what_is_published_has_none() {
    let scratch = Scratch::new("board-fonts");
    let dir = tree(&scratch);
    let fonts = scratch.dir.join("fonts");
    std::fs::create_dir_all(&fonts).unwrap();
    for (file, ..) in crate::fonts::FACES {
        std::fs::write(fonts.join(file), "").unwrap();
    }
    let plain = Source::Dir(&dir).design_system(&cache(&scratch)).unwrap();
    let drawn = Source::Dir(&dir)
        .board_design_system(&crate::Folders {
            cache: cache(&scratch),
            fonts: Some(fonts.clone()),
            volume: scratch.dir.clone(),
        })
        .unwrap();
    assert!(!plain[0].contains("@font-face"));
    assert!(drawn[0].starts_with(&plain[0]), "{}", drawn[0]);
    assert_eq!(drawn[0].matches("@font-face").count(), 12);
    assert_eq!(drawn[1..], plain[1..]);
}

#[test]
fn an_archive_that_tar_stops_reading_early_still_unpacks() {
    let scratch = Scratch::new("unpack-padding");
    let from = scratch.dir.join("from");
    std::fs::create_dir_all(&from).unwrap();
    std::fs::write(from.join("a.txt"), "a").unwrap();
    let made = std::process::Command::new("tar")
        .arg("-c")
        .arg("-C")
        .arg(&from)
        .arg("a.txt")
        .output()
        .unwrap();
    assert!(made.status.success());
    // Far more padding after the end marker than a pipe holds, which tar
    // never reads.
    let mut archive = made.stdout;
    archive.resize(archive.len() + 8 * 1024 * 1024, 0);
    let into = scratch.dir.join("into");
    std::fs::create_dir_all(&into).unwrap();
    unpack(&archive, &into, "the files", "HEAD").unwrap();
    assert_eq!(std::fs::read_to_string(into.join("a.txt")).unwrap(), "a");
}

#[test]
fn a_tar_that_fails_is_reported_whatever_the_pipe_does() {
    let scratch = Scratch::new("unpack-garbage");
    let error = message(unpack(
        &vec![7u8; 8 * 1024 * 1024],
        &scratch.dir,
        "the files",
        "HEAD",
    ));
    assert!(
        error.contains("tar failed to unpack the files of HEAD"),
        "{error}"
    );
}
