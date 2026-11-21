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

/// A design tree with a board on its canvas.
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
fn a_canvas_entry_that_names_no_board_file_fails_the_frames() {
    let root = tree();
    std::fs::write(
        root.path().join(boards::CANVAS),
        r#"{"boards": {"mac.a.b.dc.html": {"w": 690, "h": 600}, "mac.a.c.png": {"w": 1, "h": 1}}}"#,
    )
    .unwrap();
    assert!(matches!(
        frames(root.path()),
        Err(DesignError::NoBoardFile(file)) if file == "mac.a.c.png"
    ));
}

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
    assert!(!r.join("design/system/components/bundle.js").exists());
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
