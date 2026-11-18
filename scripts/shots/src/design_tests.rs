use super::*;

pub(crate) fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A design system with these built files.
pub(crate) fn system(tokens: &str, css: &str, js: &str) -> DesignSystem {
    DesignSystem {
        namespace: "demo".to_string(),
        tokens_css: tokens.to_string(),
        bundle_css: css.to_string(),
        bundle_js: js.to_string(),
    }
}

/// A design tree with a board that mounts a part that mounts another part.
fn tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    write(
        r,
        "design/mocks/mac.a.b.dc.html",
        r#"<x-dc><dc-import name="mac.parts.one" hint-size="1,2"></dc-import></x-dc>"#,
    );
    write(
        r,
        "design/mocks/mac.parts.one.dc.html",
        r#"<x-dc><dc-import hint="x" name="mac.parts.two"></dc-import></x-dc>"#,
    );
    write(r, "design/mocks/mac.parts.two.dc.html", "<x-dc>two</x-dc>");
    write(
        r,
        "design/mocks/mac.parts.other.dc.html",
        "<x-dc>other</x-dc>",
    );
    write(
        r,
        "design/mocks/canvas.json",
        r#"{"v":3,"boards":{"mac.a.b.dc.html":{"w":690,"h":600,"x":0,"y":0,"title":"t","page":"p"}}}"#,
    );
    root
}

fn hash(root: &Path) -> String {
    board_hash(
        root,
        "mac.a.b",
        &system("tokens", "css", "js"),
        "runtime",
        "chrome-1",
    )
    .unwrap()
}

#[test]
fn the_frame_comes_from_the_canvas() {
    let root = tree();
    assert_eq!(
        frame(root.path(), "mac.a.b").unwrap(),
        Frame { w: 690, h: 600 }
    );
    assert!(matches!(
        frame(root.path(), "mac.nothing"),
        Err(DesignError::NoFrame(_))
    ));
}

#[test]
fn parts_are_reached_through_parts() {
    let root = tree();
    let reached: Vec<String> = reached(root.path(), "mac.a.b")
        .unwrap()
        .into_keys()
        .collect();
    assert_eq!(reached, ["mac.a.b", "mac.parts.one", "mac.parts.two"]);
}

#[test]
fn a_part_that_is_missing_is_named() {
    let root = tree();
    std::fs::remove_file(root.path().join("design/mocks/mac.parts.two.dc.html")).unwrap();
    let error = board_hash(root.path(), "mac.a.b", &system("t", "c", "j"), "r", "c")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("mac.parts.one imports mac.parts.two"),
        "{error}"
    );
}

#[test]
fn the_hash_changes_with_a_mounted_part_and_not_with_an_unmounted_one() {
    let root = tree();
    let before = hash(root.path());
    assert_eq!(before, hash(root.path()));
    write(
        root.path(),
        "design/mocks/mac.parts.other.dc.html",
        "<x-dc>changed</x-dc>",
    );
    assert_eq!(before, hash(root.path()));
    write(
        root.path(),
        "design/mocks/mac.parts.two.dc.html",
        "<x-dc>changed</x-dc>",
    );
    assert_ne!(before, hash(root.path()));
}

#[test]
fn the_hash_changes_with_each_built_file_the_runtime_and_the_browser() {
    let root = tree();
    let r = root.path();
    let before = hash(r);
    for changed in [
        system("changed", "css", "js"),
        system("tokens", "changed", "js"),
        system("tokens", "css", "changed"),
    ] {
        assert_ne!(
            before,
            board_hash(r, "mac.a.b", &changed, "runtime", "chrome-1").unwrap()
        );
    }
    let same = system("tokens", "css", "js");
    assert_ne!(
        before,
        board_hash(r, "mac.a.b", &same, "other", "chrome-1").unwrap()
    );
    assert_ne!(
        before,
        board_hash(r, "mac.a.b", &same, "runtime", "chrome-2").unwrap()
    );
}

/// A board's hash takes the same inputs, in the same order and under the same
/// labels, as the hashes in the records repos have committed, so a record
/// stays valid for every board nothing changed. A change to the inputs
/// changes this pinned hash.
#[test]
fn the_board_hash_inputs_stay_pinned() {
    let root = tree();
    assert_eq!(hash(root.path()), PINNED_HASH);
}

const PINNED_HASH: &str = "3a5b336cf10b11bca597eed0236b2be5f8bcc6cd9ffd55aba3e13718c7d632f7";

#[test]
fn the_design_system_comes_from_the_repos_sources_and_its_canvas_namespace() {
    let root = tree();
    let r = root.path();
    write(
        r,
        "design/mocks/canvas.json",
        r#"{"designSystems":[{"namespace":"demo"}],"boards":{}}"#,
    );
    write(
        r,
        "design/system/tokens.json",
        r#"{"name":"t","color":{"themes":[{"id":"dark"}],"tokens":[]},"type":{"families":{},"groups":[]}}"#,
    );
    write(
        r,
        "design/system/components/src/bundle.js",
        "window.Demo = {};",
    );
    write(
        r,
        "design/system/build-bundle.py",
        "import pathlib\nc = pathlib.Path(__file__).with_name('components')\n\
         (c / 'bundle.js').write_text((c / 'src' / 'bundle.js').read_text())\n\
         (c / 'bundle.css').write_text('.x {}')\n",
    );
    let cache = tempfile::tempdir().unwrap();
    let design = DesignSystem::of(r, cache.path()).unwrap();
    assert_eq!(design.namespace, "demo");
    assert_eq!(design.bundle_js, "window.Demo = {};");
    assert_eq!(design.bundle_css, ".x {}");
    assert!(design.tokens_css.contains(":root, [data-theme=\"dark\"]"));
    assert!(!r.join(BUNDLE_JS).exists());
}

#[test]
fn the_ready_boards_are_the_lines_of_shots_txt() {
    let root = tree();
    write(
        root.path(),
        "design/mocks/shots.txt",
        "# ready\nmac.a.b\n\n  mac.c.d \n",
    );
    assert_eq!(ready_boards(root.path()).unwrap(), ["mac.a.b", "mac.c.d"]);
}
