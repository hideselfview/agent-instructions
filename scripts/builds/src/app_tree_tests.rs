use super::*;
use crate::app_config::{CONFIG, parse_config, resolve};
use crate::testing::{Scratch, git};

const LINES: &str = "step mac macos true\nstep android android true\non app-mac/ macos\non *.swift macos\non app-android/ android\n";

/// A repo whose `.app-build` has <LINES>, with a file in each app's folder.
fn repo(scratch: &Scratch) -> std::path::PathBuf {
    let top = scratch.dir.join("repo");
    crate::testing::repo(&top, "target\n");
    for (path, text) in [
        (CONFIG, LINES),
        ("app-mac/main.m", "mac"),
        ("app-android/Main.kt", "android"),
        ("core/lib.rs", "core"),
        ("tools/x.swift", "swift"),
    ] {
        let file = top.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    git(&top, &["add", "."]);
    git(&top, &["commit", "-q", "-m", "Apps"]);
    top
}

fn commit_change(top: &std::path::Path, path: &str, text: &str) -> String {
    std::fs::write(top.join(path), text).unwrap();
    git(top, &["commit", "-q", "-am", "Change"]);
    resolve(top, "HEAD").unwrap()
}

#[test]
fn a_lanes_app_tree_changes_only_with_the_paths_its_on_lines_name() {
    let scratch = Scratch::new("app-tree");
    let top = repo(&scratch);
    let config = parse_config(LINES).unwrap();
    let tree = |commit: &str, lane| of(&top, commit, &config, lane).unwrap();
    let first = resolve(&top, "HEAD").unwrap();
    let (mac, android) = (tree(&first, Lane::Mac), tree(&first, Lane::Android));
    assert_ne!(mac, android);
    let core = commit_change(&top, "core/lib.rs", "changed");
    assert_eq!(tree(&core, Lane::Mac), mac, "no on line names core/");
    let droid = commit_change(&top, "app-android/Main.kt", "changed");
    assert_eq!(tree(&droid, Lane::Mac), mac);
    assert_ne!(tree(&droid, Lane::Android), android);
    let swift = commit_change(&top, "tools/x.swift", "changed");
    assert_ne!(
        tree(&swift, Lane::Mac),
        mac,
        "a file the *.swift line matches"
    );
}
