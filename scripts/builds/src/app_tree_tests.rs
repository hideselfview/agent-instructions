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

/// A repo whose design system has a component preview, which draws the size
/// of a component scene, and a board, with `.app-build` <LINES>.
fn design_repo(scratch: &Scratch) -> std::path::PathBuf {
    let top = scratch.dir.join("design-repo");
    crate::testing::repo(&top, "target\n");
    for (path, text) in [
        (CONFIG, LINES),
        ("app-mac/main.m", "mac"),
        ("design/system/components/Row.preview.html", "row 273"),
        ("design/mocks/welcome.dc.html", "board"),
        ("design/mocks/runtime.toml", "runtime = \"a\""),
    ] {
        let file = top.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    git(&top, &["add", "."]);
    git(&top, &["commit", "-q", "-m", "Design"]);
    top
}

#[test]
fn a_lanes_app_tree_changes_with_what_sizes_a_component_scene() {
    let scratch = Scratch::new("app-tree-sizes");
    let top = design_repo(&scratch);
    let config = parse_config(LINES).unwrap();
    let tree = |commit: &str| of(&top, commit, &config, Lane::Mac).unwrap();
    let first = resolve(&top, "HEAD").unwrap();
    let before = tree(&first);
    let board = commit_change(&top, "design/mocks/welcome.dc.html", "another board");
    assert_eq!(
        tree(&board),
        before,
        "a screen board sizes no component scene"
    );
    let preview = commit_change(&top, "design/system/components/Row.preview.html", "row 420");
    assert_ne!(tree(&preview), before, "a preview draws the scene's size");
    let runtime = commit_change(&top, "design/mocks/runtime.toml", "runtime = \"b\"");
    assert_ne!(
        tree(&runtime),
        tree(&preview),
        "the runtime draws the scene"
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
        let commit = commit_change(
            &top,
            path,
            &(std::fs::read_to_string(top.join(path)).unwrap() + "\n# changed\n"),
        );
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

#[test]
fn the_picture_folder_id_differs_between_environments_for_one_app_tree() {
    let tree = "0123456789abcdef";
    let one = in_environment(tree, "26B131-17C529");
    let other = in_environment(tree, "25G241-27B52");
    assert_eq!(one.len(), 16);
    assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(one, other);
    assert_ne!(one, tree);
    assert_eq!(one, in_environment(tree, "26B131-17C529"));
    assert_ne!(
        in_environment("fedcba9876543210", "26B131-17C529"),
        one,
        "another app tree has another id in the same environment"
    );
}
