use super::*;
use crate::design_canvas::tests::{DS, ONE, TWO, boards, canvases_toml, index};
use crate::testing::{Scratch, design_system, git, git_out, repo};

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().text()
}

const RUNTIME_SHA: &str = "b795ee21261f2e7da1b10cb25b4a4cfa275d1b96165f89396f1fed426e191844";

/// A repo with the demo boards, index and canvases committed, with a README
/// beside them that is no board, a design system with two components and a
/// brand book, and the record of the canvas runtime.
fn demo(scratch: &Scratch) -> PathBuf {
    let dir = scratch.dir.join("repo");
    repo(&dir, "");
    let mocks = dir.join(MOCKS);
    std::fs::create_dir_all(&mocks).unwrap();
    design_system(
        &dir,
        "1",
        ".bundle { color: red; }",
        "window.Bae = { BUNDLE_MARK: 1 };",
    );
    let system = dir.join(SYSTEM);
    for (path, text) in [
        ("README.md", "# Demo\n"),
        ("components/index.d.ts", "export {};\n"),
        ("components/MacButton/README.md", "A button.\n"),
        ("components/MacButton/preview.html", "<!-- @dsCard -->\n"),
        ("components/Cover/preview.html", "<!-- @dsCard -->\n"),
    ] {
        write(&system.join(path), text).unwrap();
    }
    for (name, text) in boards() {
        std::fs::write(mocks.join(name), text).unwrap();
    }
    std::fs::write(mocks.join("README.md"), "# Mocks\n").unwrap();
    std::fs::write(mocks.join("canvas.json"), index().to_string()).unwrap();
    std::fs::write(dir.join(CANVASES), canvases_toml()).unwrap();
    std::fs::write(
        dir.join(RUNTIME),
        format!(
            "canvas = \"{ONE}\"\nruntime = \"artifact-type/dc-runtime.js\"\nruntime_sha256 = \"{RUNTIME_SHA}\"\n"
        ),
    )
    .unwrap();
    commit(&dir);
    dir
}

fn commit(dir: &Path) {
    git(dir, &["add", "design"]);
    git(dir, &["commit", "-q", "-m", "boards"]);
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn json_of(path: &Path) -> Value {
    serde_json::from_str(&read(path)).unwrap()
}

/// Every file under <dir>, by its path relative to <dir>.
fn files_under(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(next) = todo.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                todo.push(path);
            } else {
                found.push(
                    path.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    found.sort();
    found
}

/// The paths every call file of <calls> publishes, in order: each call's
/// file_path (under <root>), then its files.
fn sent(root: &Path, calls: &[PathBuf]) -> Vec<String> {
    let mut paths = Vec::new();
    for call in calls {
        let call = json_of(call);
        let file_path = call["file_path"].as_str().unwrap();
        paths.push(
            Path::new(file_path)
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        for (to, from) in call["files"].as_object().unwrap() {
            assert_eq!(
                Some(to.as_str()),
                from.as_str(),
                "a file is sent from its own path"
            );
            paths.push(to.clone());
        }
    }
    paths
}

fn write_demo(scratch: &Scratch, dir: &Path, rev: &str) -> Publish {
    write_all(
        dir,
        rev,
        &scratch.dir.join("out"),
        &scratch.dir.join("cache"),
    )
    .unwrap()
}

/// Each canvas gets the boards on its pages, the index of only those boards
/// and notes with its own title and launch page, and a copy of the design
/// system; the design system's artifact gets its brand book, tokens and
/// components with the same tokens and bundles; and every folder says the
/// one commit its files come from.
#[test]
fn every_artifact_gets_its_files_from_one_commit() {
    let scratch = Scratch::new("publish-files");
    let dir = demo(&scratch);
    let head = git_out(&dir, &["rev-parse", "HEAD"]);
    let publish = write_demo(&scratch, &dir, "main");
    assert_eq!(publish.commit, head);
    assert_eq!(
        publish
            .artifacts
            .iter()
            .map(|w| (w.url.as_str(), w.kind))
            .collect::<Vec<_>>(),
        [
            (ONE, Kind::Canvas),
            (TWO, Kind::Canvas),
            (DS, Kind::DesignSystem)
        ]
    );
    let out = std::fs::canonicalize(scratch.dir.join("out")).unwrap();
    let one = out.join("CanvasOne");
    let two = out.join("CanvasTwo");
    let ds = out.join("DesignSystem");
    assert_eq!(publish.artifacts[0].root, one);
    let copy = [
        "project/ds/demo/components/bundle.css",
        "project/ds/demo/components/bundle.js",
        "project/ds/demo/tokens.css",
        "project/ds/demo/tokens.json",
    ];
    let mut expected_one = vec![
        "project/a.dc.html",
        "project/c.dc.html",
        "project/canvas.json",
    ];
    expected_one.extend(copy);
    expected_one.sort();
    let mut with_record = expected_one.clone();
    with_record.extend(["artifact.json", "call-1.json"]);
    with_record.sort();
    assert_eq!(files_under(&one), with_record);
    let mut expected_two = vec![
        "artifact.json",
        "call-1.json",
        "project/b.dc.html",
        "project/canvas.json",
        "project/d.dc.html",
    ];
    expected_two.extend(copy);
    expected_two.sort();
    assert_eq!(files_under(&two), expected_two);
    let expected_ds = [
        "artifact.json",
        "call-1.json",
        "project/README.md",
        "project/components/Cover/preview.html",
        "project/components/MacButton/README.md",
        "project/components/MacButton/preview.html",
        "project/components/bundle.css",
        "project/components/bundle.js",
        "project/components/index.d.ts",
        "project/components/src/bundle.css",
        "project/components/src/bundle.js",
        "project/tokens.css",
        "project/tokens.json",
    ];
    assert_eq!(files_under(&ds), expected_ds);

    assert_eq!(read(&one.join("project/a.dc.html")), boards()["a.dc.html"]);
    for rel in DS_FILES {
        assert_eq!(
            read(&one.join(format!("project/ds/demo/{rel}"))),
            read(&ds.join(format!("project/{rel}"))),
            "the canvas's copy of {rel} is the design system's"
        );
    }
    assert_eq!(
        read(&ds.join("project/components/bundle.js")),
        "window.Bae = { BUNDLE_MARK: 1 };"
    );
    assert!(read(&ds.join("project/tokens.css")).contains("--tokens: 1"));
    assert_eq!(
        read(&ds.join("project/tokens.json")),
        read(&dir.join("design/system/tokens.json"))
    );
    assert_eq!(read(&ds.join("project/README.md")), "# Demo\n");

    let index_one = json_of(&one.join("project/canvas.json"));
    let mut expected = index();
    expected["title"] = json!("demo: one");
    expected["launch"] = json!({"view": "canvas", "page": "one"});
    expected["pages"] = json!([{"id": "one", "name": "One"}]);
    expected["boards"] = json!({
        "a.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10, "page": "one"},
        "c.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10}
    });
    expected["order"] = json!(["c.dc.html", "a.dc.html"]);
    expected["notes"] = json!({"n2": {"x": 0, "y": 0, "text": "on the first page"}});
    expected["noteOrder"] = json!(["n2"]);
    assert_eq!(index_one, expected);
    let index_two = json_of(&two.join("project/canvas.json"));
    assert_eq!(index_two["title"], json!("demo: two and three"));
    assert_eq!(
        index_two["launch"],
        json!({"view": "canvas", "page": "three"})
    );
    assert_eq!(index_two["order"], json!(["d.dc.html", "b.dc.html"]));
    assert_eq!(index_two["noteOrder"], json!(["n1"]));

    let call = json_of(&one.join("call-1.json"));
    assert_eq!(call["url"], json!(ONE));
    assert_eq!(call["root"], json!(one.display().to_string()));
    assert_eq!(
        call["file_path"],
        json!(one.join("project/canvas.json").display().to_string()),
        "the index goes first"
    );
    let mut one_sent = sent(&one, &publish.artifacts[0].calls);
    one_sent.sort();
    assert_eq!(one_sent, expected_one);
    for (root, kind) in [(&one, "canvas"), (&ds, "design-system")] {
        let record = json_of(&root.join(RECORD));
        assert_eq!(record["commit"], json!(head));
        assert_eq!(record["kind"], json!(kind));
    }

    let said = report(&publish);
    for part in [
        ONE,
        TWO,
        DS,
        &head,
        &one.display().to_string(),
        &one.join("call-1.json").display().to_string(),
        &ds.join("call-1.json").display().to_string(),
        "7 files",
        "artifact-type/dc-runtime.js",
        RUNTIME_SHA,
    ] {
        assert!(said.contains(part), "the report names {part}: {said}");
    }
}

/// A folder that already holds files is refused, so no file left from an
/// earlier run is published.
#[test]
fn an_out_folder_that_holds_files_is_refused() {
    let scratch = Scratch::new("publish-files-out");
    let dir = demo(&scratch);
    let out = scratch.dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("left.txt"), "x").unwrap();
    assert!(
        message(write_all(&dir, "HEAD", &out, &scratch.dir.join("cache"))).contains("is not empty")
    );
}

/// A repo with no canvases file or no runtime record publishes nothing, and
/// says which file it reads.
#[test]
fn a_repo_without_a_canvases_file_or_runtime_record_is_refused() {
    for file in [CANVASES, RUNTIME] {
        let scratch = Scratch::new("publish-files-none");
        let dir = demo(&scratch);
        git(&dir, &["rm", "-q", file]);
        git(&dir, &["commit", "-q", "-m", "no file"]);
        let refused = message(write_all(
            &dir,
            "HEAD",
            &scratch.dir.join("out"),
            &scratch.dir.join("cache"),
        ));
        assert!(refused.contains(file), "{refused}");
        assert!(!scratch.dir.join("out").exists(), "nothing is written");
    }
}

/// No call sends more files than one publish does. A canvas's index goes
/// first, as the file_path of its first call, with the design system's copy;
/// the design system's tokens and bundles go first and its cover last.
#[test]
fn calls_hold_at_most_one_publishs_files_in_order() {
    let scratch = Scratch::new("publish-calls");
    let calls_of = |kind: Kind, many: &str, first: &[&str], last: &str| {
        let mut files: BTreeMap<String, String> = (0..300)
            .map(|n| (format!("{many}{n:03}.html"), String::new()))
            .collect();
        for path in first.iter().chain([&last]) {
            files.insert(path.to_string(), String::new());
        }
        let root = scratch.dir.join(format!("{kind:?}"));
        std::fs::create_dir_all(&root).unwrap();
        let url = if kind == Kind::Canvas { ONE } else { DS };
        let written = write_artifact(&root, url, kind, "abc", &files).unwrap();
        let calls: Vec<Vec<String>> = written
            .calls
            .iter()
            .map(|call| sent(&written.root, std::slice::from_ref(call)))
            .collect();
        assert_eq!(
            calls.iter().map(Vec::len).collect::<Vec<_>>(),
            [BATCH, files.len() - BATCH]
        );
        for path in first {
            assert!(calls[0].iter().any(|p| p == path), "{path} goes first");
        }
        assert!(calls[1].iter().any(|p| p == last), "{last} goes last");
        calls
    };
    let calls = calls_of(
        Kind::Canvas,
        "project/b",
        &[PUBLISHED_INDEX, "project/ds/demo/tokens.json"],
        "project/zz.dc.html",
    );
    assert_eq!(calls[0][0], PUBLISHED_INDEX);
    calls_of(
        Kind::DesignSystem,
        "project/components/A",
        &[
            "project/tokens.json",
            "project/tokens.css",
            "project/components/bundle.css",
            "project/components/bundle.js",
        ],
        "project/components/Cover/preview.html",
    );
}

/// The Artifact tool's listing of <paths>, as it prints one.
fn listing(url: &str, paths: &[&str]) -> String {
    let mut out = format!(
        "Published files of {url} (version 1-a), {} files by path:\n",
        paths.len()
    );
    for path in paths {
        out.push_str(&format!("- \"{path}\"  text/html  10 bytes  sha256 abc…\n"));
    }
    out.push_str("[Stored for the live version: contract 0.2.47]\n");
    out
}

/// A canvas's last call removes each file under project/ that the listing
/// shows and the commit lacks, and leaves the type's files; a canvas the
/// commit matches needs no last call.
#[test]
fn a_canvas_last_call_removes_what_the_commit_lacks() {
    let scratch = Scratch::new("publish-last-canvas");
    let dir = demo(&scratch);
    let publish = write_demo(&scratch, &dir, "HEAD");
    let one = &publish.artifacts[0].root;
    let shown = listing(
        ONE,
        &[
            "index.html",
            "artifact-type/dc-runtime.js",
            "project/canvas.json",
            "project/a.dc.html",
            "project/gone.dc.html",
            "project/ds/demo/old.css",
        ],
    );
    let calls = write_last(one, &shown, None, "now").unwrap();
    assert_eq!(calls, [one.join("last-1.json")]);
    let call = json_of(&calls[0]);
    assert_eq!(call["url"], json!(ONE));
    assert_eq!(
        call["file_path"],
        json!(one.join(PUBLISHED_INDEX).display().to_string())
    );
    assert_eq!(
        call["files"],
        json!({"project/ds/demo/old.css": null, "project/gone.dc.html": null})
    );
    assert!(last_report(one, &calls).contains("last-1.json"));

    let calls = write_last(one, &listing(ONE, &["project/a.dc.html"]), None, "now").unwrap();
    assert!(calls.is_empty());
    assert!(
        !one.join("last-1.json").exists(),
        "a later run replaces the earlier calls"
    );
    assert!(last_report(one, &calls).contains("needs no last call"));

    let refused = message(write_last(one, &shown, Some(("{}", "Dima")), "now"));
    assert!(refused.contains("takes no index"), "{refused}");
    let refused = message(write_last(one, "nothing here\n", None, "now"));
    assert!(refused.contains("names no file"), "{refused}");
}

/// The design system's last calls remove what the commit lacks, but never
/// the page's own files, and send its index last, with the commit in its
/// lastChange.
#[test]
fn the_design_systems_last_call_sends_its_index_last() {
    let scratch = Scratch::new("publish-last-ds");
    let dir = demo(&scratch);
    let publish = write_demo(&scratch, &dir, "HEAD");
    let ds = &publish.artifacts[2].root;
    let shown = listing(
        DS,
        &[
            "SKILL.md",
            "project/design-system.json",
            "project/manifest.json",
            "project/api/tokens.md",
            "project/assets/Logos/README.md",
            "project/README.md",
            "project/components/OldThing/README.md",
        ],
    );
    let refused = message(write_last(ds, &shown, None, "now"));
    assert!(refused.contains("--index"), "{refused}");
    let live = r#"{"v":3,"createdOnFiles":{"v":1,"at":"x"},"title":"demo"}"#;
    let calls = write_last(ds, &shown, Some((live, "Dima")), "2026-11-18T19:00:00Z").unwrap();
    assert_eq!(calls, [ds.join("last-1.json"), ds.join("last-2.json")]);
    let removal = json_of(&calls[0]);
    assert_eq!(
        removal["files"],
        json!({"project/components/OldThing/README.md": null})
    );
    assert_eq!(
        removal["file_path"],
        json!(ds.join("project/README.md").display().to_string())
    );
    let last = json_of(&calls[1]);
    assert_eq!(
        last["file_path"],
        json!(ds.join(system::INDEX).display().to_string())
    );
    assert_eq!(last["files"], json!({}));
    let index = json_of(&ds.join(system::INDEX));
    assert_eq!(index["title"], json!("demo"));
    assert_eq!(index["lastChange"]["by"], json!("Dima"));
    assert_eq!(index["lastChange"]["at"], json!("2026-11-18T19:00:00Z"));
    assert!(
        index["lastChange"]["note"]
            .as_str()
            .unwrap()
            .contains(&publish.commit)
    );
}
