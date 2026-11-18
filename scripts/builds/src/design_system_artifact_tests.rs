use super::*;
use crate::testing::Scratch;

/// The files under `project/` that the page writes or keeps itself stay on
/// the artifact whatever the repo holds; the repo decides every other file
/// under `project/`, and nothing outside it.
#[test]
fn the_repo_decides_every_project_file_but_the_pages_own() {
    for path in [
        "project/README.md",
        "project/tokens.json",
        "project/components/MacButton/preview.html",
        "project/archived/old.md",
        "project/assets/Logos/README.md",
    ] {
        assert!(repo_decides(path), "{path}");
    }
    for path in [
        INDEX,
        "project/manifest.json",
        "project/api/components/MacButton.md",
        "project/.image-slots.state.json",
        "index.html",
        "SKILL.md",
        "artifact-type/app.js",
    ] {
        assert!(!repo_decides(path), "{path}");
    }
}

/// The index keeps every key the page wrote, and its lastChange names who
/// published which commit, and when.
#[test]
fn the_index_keeps_its_keys_and_says_which_commit_it_holds() {
    let live = r#"{"v":3,"layout":"files","createdOnFiles":{"v":1,"at":"2026-11-05T15:42:55Z"},"title":"bae","blobs":{"x":1},"lastChange":{"by":"old"}}"#;
    let mut index = parse_index(live).unwrap();
    set_last_change(&mut index, "Dima", "2026-11-18T19:00:00Z", "abc123");
    let written: Value = serde_json::from_str(&index_text(&index)).unwrap();
    let mut expected: Value = serde_json::from_str(live).unwrap();
    expected["lastChange"] = json!({
        "by": "Dima",
        "at": "2026-11-18T19:00:00Z",
        "via": "Claude Code",
        "note": "Published from commit abc123 of the repo's design/system.",
    });
    assert_eq!(written, expected);
}

/// A file without the artifact type's marker is no index, and builds writes
/// nothing over it.
#[test]
fn a_file_without_the_marker_is_no_index() {
    for text in [r#"{"v":3,"title":"bae"}"#, "[]", "not json"] {
        assert!(parse_index(text).is_err(), "{text}");
    }
    assert!(parse_index(r#"{"convertedFrom":{}}"#).is_ok());
}

/// A component folder whose name the artifact type refuses is refused here,
/// naming it, before anything is published.
#[test]
fn a_component_folder_that_is_no_identifier_is_refused() {
    let scratch = Scratch::new("ds-files");
    let system = scratch.dir.join(SYSTEM);
    std::fs::create_dir_all(system.join("components/Mac-Button")).unwrap();
    std::fs::write(system.join("README.md"), "# Demo\n").unwrap();
    std::fs::write(system.join("tokens.json"), "{}").unwrap();
    let generated = ["css".to_string(), "bcss".to_string(), "bjs".to_string()];
    let refused = contents(&scratch.dir, &generated).unwrap_err().text();
    assert!(refused.contains("components/Mac-Button"), "{refused}");
    std::fs::rename(
        system.join("components/Mac-Button"),
        system.join("components/MacButton"),
    )
    .unwrap();
    std::fs::write(system.join("components/MacButton/README.md"), "Button.\n").unwrap();
    std::fs::create_dir_all(system.join("assets/Logos")).unwrap();
    std::fs::write(system.join("assets/Logos/README.md"), "Logos.\n").unwrap();
    std::fs::write(system.join("assets/Logos/mark.svg"), "<svg/>").unwrap();
    let Contents {
        files: published,
        uploads,
    } = contents(&scratch.dir, &generated).unwrap();
    assert_eq!(
        uploads
            .iter()
            .map(|(asset, _)| asset.path.as_str())
            .collect::<Vec<_>>(),
        ["project/assets/Logos/mark.svg"]
    );
    assert_eq!(
        published.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "project/README.md",
            "project/assets/Logos/README.md",
            "project/components/MacButton/README.md",
            "project/components/bundle.css",
            "project/components/bundle.js",
            "project/tokens.css",
            "project/tokens.json",
        ]
    );
    assert_eq!(published["project/components/bundle.js"], "bjs");
}

/// The shared files go in the first call and the cover in the last.
#[test]
fn the_shared_files_go_first_and_the_cover_last() {
    for rel in SHARED_FILES {
        assert_eq!(rank(&format!("project/{rel}")), 0, "{rel}");
    }
    assert_eq!(rank("project/README.md"), 1);
    assert_eq!(rank("project/components/MacButton/preview.html"), 1);
    assert_eq!(rank("project/components/Cover/preview.html"), 2);
}

/// A new design system's index takes its namespace from the bundle's header,
/// the first line, and a bundle without one is refused.
#[test]
fn the_bundles_header_names_the_namespace() {
    let bundle = "/* @ds-bundle: {\"format\":4,\"namespace\":\"Bae\",\"components\":[]} */\nwindow.Bae = {};\n";
    assert_eq!(bundle_namespace(bundle).unwrap(), "Bae");
    for bundle in [
        "window.Bae = {};\n",
        "/* @ds-bundle: {\"format\":4} */\n",
        "/* @ds-bundle: not json */\n",
    ] {
        assert!(bundle_namespace(bundle).is_err(), "{bundle}");
    }
    let index = start_index("bae", "Bae", "2026-11-18T19:00:00Z");
    let text = index_text(&index);
    let parsed = parse_index(&text).unwrap();
    assert_eq!(parsed["title"], json!("bae"));
    assert_eq!(parsed["namespace"], json!("Bae"));
}
