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
        fonts: PathBuf::new(),
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

fn hash_with(
    root: &Path,
    design: &DesignSystem,
    runtime: &str,
    browser: &str,
    program: &str,
    settings: &Settings,
) -> String {
    board_hash(
        root,
        "mac.a.b",
        runtime,
        &Common {
            design,
            browser_version: browser,
            program_version: program,
            settings,
            scale: 2.0,
        },
    )
    .unwrap()
}

fn hash(root: &Path) -> String {
    hash_with(
        root,
        &system("tokens", "css", "js"),
        "runtime",
        "chrome-1",
        "program-1",
        &Settings::default(),
    )
}

#[test]
fn a_board_hash_depends_on_the_scale() {
    let root = tree();
    let design = system("tokens", "css", "js");
    let at = |scale: f64| {
        board_hash(
            root.path(),
            "mac.a.b",
            "runtime",
            &Common {
                design: &design,
                browser_version: "chrome-1",
                program_version: "program-1",
                settings: &Settings::default(),
                scale,
            },
        )
        .unwrap()
    };
    assert_eq!(at(2.0), at(2.0));
    assert_ne!(at(1.0), at(2.0));
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
    let error = board_hash(
        root.path(),
        "mac.a.b",
        "r",
        &Common {
            design: &system("t", "c", "j"),
            browser_version: "c",
            program_version: "p",
            settings: &Settings::default(),
            scale: 2.0,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("mac.parts.one imports mac.parts.two"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn a_part_whose_metadata_cannot_be_read_reports_the_read_error() {
    let root = tree();
    let path = board_file(root.path(), "mac.parts.two");
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&path, &path).unwrap();

    let error = reached(root.path(), "mac.a.b").unwrap_err();
    assert!(
        matches!(error, DesignError::Unreadable { path: ref found, .. } if found == &path),
        "{error}"
    );
}

#[test]
fn the_board_hash_changes_with_a_mounted_part_and_not_with_an_unmounted_one() {
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
fn the_board_hash_changes_with_the_frame_each_built_file_the_runtime_the_browser_the_program_and_the_unshot_parts()
 {
    let root = tree();
    let r = root.path();
    let before = hash(r);
    let same = system("tokens", "css", "js");
    let plain = Settings::default();
    for changed in [
        system("changed", "css", "js"),
        system("tokens", "changed", "js"),
        system("tokens", "css", "changed"),
    ] {
        assert_ne!(
            before,
            hash_with(r, &changed, "runtime", "chrome-1", "program-1", &plain)
        );
    }
    assert_ne!(
        before,
        hash_with(r, &same, "other", "chrome-1", "program-1", &plain)
    );
    assert_ne!(
        before,
        hash_with(r, &same, "runtime", "chrome-2", "program-1", &plain)
    );
    assert_ne!(
        before,
        hash_with(r, &same, "runtime", "chrome-1", "program-2", &plain)
    );
    let unshot = Settings {
        unshot: vec![".keyboard".to_string()],
        ..Settings::default()
    };
    assert_ne!(
        before,
        hash_with(r, &same, "runtime", "chrome-1", "program-1", &unshot)
    );
    write(
        r,
        "design/mocks/canvas.json",
        r#"{"v":3,"boards":{"mac.a.b.dc.html":{"w":700,"h":600,"x":0,"y":0,"title":"t","page":"p"}}}"#,
    );
    assert_ne!(before, hash(r));
}

#[test]
fn a_preview_hash_names_its_shot_so_each_row_of_a_preview_has_its_own() {
    let root = tree();
    let r = root.path();
    write(
        r,
        &boards::preview::preview_path("MacRow"),
        "<x-preview></x-preview>",
    );
    let design = system("tokens", "css", "js");
    let of = |shot: &str, settings: &Settings| {
        preview_hash(
            r,
            shot,
            "MacRow",
            &Common {
                design: &design,
                browser_version: "chrome-1",
                program_version: "program-1",
                settings,
                scale: 2.0,
            },
        )
        .unwrap()
    };
    let plain = Settings::default();
    let label = of("mac.component.row.label", &plain);
    assert_eq!(label, of("mac.component.row.label", &plain));
    assert_ne!(label, of("mac.component.row.value", &plain));
    let marked = Settings {
        preview: Some(crate::settings::Preview {
            row: ".row".to_string(),
            name: ".name".to_string(),
            cell: ".cell".to_string(),
        }),
        ..Settings::default()
    };
    assert_ne!(label, of("mac.component.row.label", &marked));
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
        r#"{"name":"t","color":{"themes":[{"id":"dark","name":"Dark"}],"tokens":[]},"type":{"families":{},"groups":[]}}"#,
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
    let design = DesignSystem::of(
        r,
        Folders {
            cache: cache.path(),
            fonts: &cache.path().join("no-fonts"),
        },
    )
    .unwrap();
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
