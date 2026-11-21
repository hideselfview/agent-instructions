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

const RUST_LINES: &str = "step mac macos true\non app-mac/ macos\non bridge/ macos\n";

/// A repo of the crates bridge (which depends on core through the
/// workspace table and on util by path, and whose script generates with
/// gen), core, util, gen and other, a Cargo.lock and a toolchain pin.
fn rust_repo(scratch: &Scratch) -> std::path::PathBuf {
    let top = scratch.dir.join("rust-repo");
    crate::testing::repo(&top, "target\n");
    let manifest = |name: &str, deps: &str| {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n{deps}")
    };
    for (path, text) in [
        (CONFIG, RUST_LINES.to_string()),
        ("app-mac/main.m", "mac".into()),
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"bridge\", \"core\", \"util\", \"gen\", \"other\"]\n\n[workspace.dependencies]\ncore = { path = \"core\" }\n".into(),
        ),
        ("Cargo.lock", "lock".into()),
        ("rust-toolchain.toml", "stable".into()),
        (
            "bridge/Cargo.toml",
            manifest("bridge", "[dependencies]\ncore = { workspace = true }\n\n[target.'cfg(unix)'.dependencies]\nutil = { path = \"../util\" }\n"),
        ),
        ("bridge/build.sh", "cargo run -p gen --bin make\n".into()),
        ("bridge/src/lib.rs", "bridge".into()),
        ("core/Cargo.toml", manifest("core", "")),
        ("core/src/lib.rs", "core".into()),
        ("util/Cargo.toml", manifest("util", "")),
        ("util/src/lib.rs", "util".into()),
        ("gen/Cargo.toml", manifest("gen", "")),
        ("gen/src/main.rs", "gen".into()),
        ("other/Cargo.toml", manifest("other", "")),
        ("other/src/lib.rs", "other".into()),
    ] {
        let file = top.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    git(&top, &["add", "."]);
    git(&top, &["commit", "-q", "-m", "Crates"]);
    top
}

#[test]
fn a_lanes_app_tree_holds_the_crates_its_bridge_builds_from() {
    let scratch = Scratch::new("app-tree-crates");
    let top = rust_repo(&scratch);
    let config = parse_config(RUST_LINES).unwrap();
    let tree = |commit: &str| of(&top, commit, &config, Lane::Mac).unwrap();
    let first = resolve(&top, "HEAD").unwrap();
    let base = tree(&first);
    let mut seen = vec![base.clone()];
    for (path, why) in [
        ("core/src/lib.rs", "a crate in the workspace table"),
        ("util/src/lib.rs", "a path dependency under a target table"),
        ("gen/src/main.rs", "a crate a script of the bridge builds"),
        ("Cargo.lock", "the lock"),
        ("rust-toolchain.toml", "the toolchain pin"),
        ("Cargo.toml", "the workspace manifest"),
    ] {
        let commit = commit_change(&top, path, "# changed\n");
        let now = tree(&commit);
        assert!(!seen.contains(&now), "{path}: {why}");
        seen.push(now);
    }
}

#[test]
fn a_crate_the_bridge_does_not_use_leaves_the_app_tree_alone() {
    let scratch = Scratch::new("app-tree-unrelated");
    let top = rust_repo(&scratch);
    let config = parse_config(RUST_LINES).unwrap();
    let first = resolve(&top, "HEAD").unwrap();
    let base = of(&top, &first, &config, Lane::Mac).unwrap();
    let unrelated = commit_change(&top, "other/src/lib.rs", "changed");
    assert_eq!(of(&top, &unrelated, &config, Lane::Mac).unwrap(), base);
}
