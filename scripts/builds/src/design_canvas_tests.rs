use super::*;
use crate::testing::{Scratch, design_system, git, repo};
use serde_json::json;
use std::path::PathBuf;

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().text()
}

const ONE: &str = "https://claude.ai/artifact/CanvasOne";
const TWO: &str = "https://claude.ai/artifact/CanvasTwo";

/// The canvases of the demo repo: page `one` on the first canvas, pages `two`
/// and `three` on the second.
fn canvases_toml() -> String {
    format!(
        "# The canvases.\n\n[[canvas]]\nurl = \"{ONE}\"\ntitle = \"demo: one\"\nlaunch = \"one\"\npages = [\"one\"]\n\n\
         [[canvas]]\nurl = \"{TWO}\"\ntitle = \"demo: two and three\"\nlaunch = \"three\"\npages = [\"two\", \"three\"]\n"
    )
}

/// The demo repo's canvas index: board `a` on page one mounts `c`, which has
/// no page and so is on the first page; `b` is on page two and `d` on page
/// three. Note `n1` is on page two and `n2` has no page.
fn index() -> Value {
    json!({
        "v": 3,
        "title": "demo mocks",
        "attachments": {},
        "createdOnFiles": {"v": 1, "at": "2026-11-07T03:22:02Z"},
        "designSystems": [{"title": "demo", "namespace": "demo", "artifact": "https://claude.ai/artifact/Ds", "version": null, "copiedAt": "2026-11-07T03:22:02Z"}],
        "launch": {"view": "canvas", "page": "two"},
        "pages": [{"id": "one", "name": "One"}, {"id": "two", "name": "Two"}, {"id": "three", "name": "Three"}],
        "boards": {
            "a.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10, "page": "one"},
            "b.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10, "page": "two"},
            "c.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10},
            "d.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10, "page": "three"}
        },
        "order": ["d.dc.html", "c.dc.html", "b.dc.html", "a.dc.html"],
        "notes": {
            "n1": {"x": 0, "y": 0, "text": "on two", "page": "two"},
            "n2": {"x": 0, "y": 0, "text": "on the first page"}
        },
        "noteOrder": ["n2", "n1"]
    })
}

/// The files of the demo repo's design/mocks, by name.
fn boards() -> BTreeMap<String, String> {
    [
        (
            "a.dc.html",
            "<x-dc>A <dc-import name=\"c\"></dc-import></x-dc>",
        ),
        ("b.dc.html", "<x-dc>B</x-dc>"),
        ("c.dc.html", "<x-dc>C</x-dc>"),
        ("d.dc.html", "<x-dc>D</x-dc>"),
    ]
    .into_iter()
    .map(|(name, text)| (name.to_string(), text.to_string()))
    .collect()
}

/// A repo with the demo boards, index and canvases committed, with a README
/// beside them that is no board, and a design system.
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
    for (name, text) in boards() {
        std::fs::write(mocks.join(name), text).unwrap();
    }
    std::fs::write(mocks.join("README.md"), "# Mocks\n").unwrap();
    std::fs::write(mocks.join("canvas.json"), index().to_string()).unwrap();
    std::fs::write(dir.join(CANVASES), canvases_toml()).unwrap();
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

/// Each canvas gets the boards on its pages, the index of only those boards
/// and notes with its own title and launch page, and a copy of the design
/// system at the commit, in a folder of its own, with the batches that
/// publish every file of the folder.
#[test]
fn each_canvas_gets_its_pages_boards_and_the_design_system() {
    let scratch = Scratch::new("canvas-files");
    let dir = demo(&scratch);
    let out = scratch.dir.join("out");
    let written = write_canvases(&dir, "HEAD", &out, &scratch.dir.join("cache")).unwrap();
    assert_eq!(
        written.iter().map(|w| w.url.as_str()).collect::<Vec<_>>(),
        [ONE, TWO]
    );
    let one = out.join("CanvasOne");
    let two = out.join("CanvasTwo");
    assert_eq!(written[0].root, one);
    let ds = [
        "project/ds/demo/components/bundle.css",
        "project/ds/demo/components/bundle.js",
        "project/ds/demo/tokens.css",
        "project/ds/demo/tokens.json",
    ];
    let mut expected_one = vec![
        "batch-1.json",
        "project/a.dc.html",
        "project/c.dc.html",
        "project/canvas.json",
    ];
    expected_one.extend(ds);
    expected_one.sort();
    assert_eq!(files_under(&one), expected_one);
    let mut expected_two = vec![
        "batch-1.json",
        "project/b.dc.html",
        "project/canvas.json",
        "project/d.dc.html",
    ];
    expected_two.extend(ds);
    expected_two.sort();
    assert_eq!(files_under(&two), expected_two);

    assert_eq!(read(&one.join("project/a.dc.html")), boards()["a.dc.html"]);
    assert_eq!(
        read(&one.join("project/ds/demo/components/bundle.js")),
        "window.Bae = { BUNDLE_MARK: 1 };"
    );
    assert!(read(&one.join("project/ds/demo/tokens.css")).contains("--tokens: 1"));
    assert_eq!(
        read(&one.join("project/ds/demo/tokens.json")),
        read(&dir.join("design/system/tokens.json"))
    );

    let index_one: Value = serde_json::from_str(&read(&one.join("project/canvas.json"))).unwrap();
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
    let index_two: Value = serde_json::from_str(&read(&two.join("project/canvas.json"))).unwrap();
    assert_eq!(index_two["title"], json!("demo: two and three"));
    assert_eq!(
        index_two["launch"],
        json!({"view": "canvas", "page": "three"})
    );
    assert_eq!(index_two["order"], json!(["d.dc.html", "b.dc.html"]));
    assert_eq!(index_two["noteOrder"], json!(["n1"]));

    let batch: BTreeMap<String, String> =
        serde_json::from_str(&read(&one.join("batch-1.json"))).unwrap();
    let published: Vec<&String> = batch.keys().collect();
    assert_eq!(
        published,
        expected_one
            .iter()
            .filter(|path| path.starts_with("project/"))
            .collect::<Vec<_>>()
    );
    assert!(batch.iter().all(|(to, from)| to == from));
    assert_eq!(written[0].batches, [one.join("batch-1.json")]);

    let said = report(&written);
    for part in [
        ONE,
        TWO,
        &one.display().to_string(),
        &one.join("batch-1.json").display().to_string(),
        "7 files",
    ] {
        assert!(said.contains(part), "the report names {part}: {said}");
    }
}

/// A folder that already holds files is refused, so no file left from an
/// earlier run is published.
#[test]
fn an_out_folder_that_holds_files_is_refused() {
    let scratch = Scratch::new("canvas-files-out");
    let dir = demo(&scratch);
    let out = scratch.dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("left.txt"), "x").unwrap();
    assert!(
        message(write_canvases(
            &dir,
            "HEAD",
            &out,
            &scratch.dir.join("cache")
        ))
        .contains("is not empty")
    );
}

/// A repo with no canvases file publishes none, and says which file it reads.
#[test]
fn a_repo_without_a_canvases_file_is_refused() {
    let scratch = Scratch::new("canvas-files-none");
    let dir = demo(&scratch);
    git(&dir, &["rm", "-q", CANVASES]);
    git(&dir, &["commit", "-q", "-m", "no canvases"]);
    let refused = message(write_canvases(
        &dir,
        "HEAD",
        &scratch.dir.join("out"),
        &scratch.dir.join("cache"),
    ));
    assert!(refused.contains(CANVASES), "{refused}");
}

/// The canvases parsed from <text>.
fn parsed(text: &str) -> Vec<Canvas> {
    parse_canvases(text, CANVASES).unwrap()
}

#[test]
fn a_split_that_leaves_a_page_out_or_gives_it_twice_is_refused() {
    let split_of = |text: &str| message(split(&index(), &parsed(text), &boards()));
    let canvas = |url: &str, launch: &str, pages: &str| {
        format!(
            "[[canvas]]\nurl = \"{url}\"\ntitle = \"t\"\nlaunch = \"{launch}\"\npages = [{pages}]\n"
        )
    };
    let refused = split_of(&canvas(ONE, "one", "\"one\", \"two\""));
    assert!(refused.contains("page three is on no canvas"), "{refused}");
    let refused = split_of(&format!(
        "{}{}",
        canvas(ONE, "one", "\"one\", \"two\""),
        canvas(TWO, "two", "\"two\", \"three\"")
    ));
    assert!(refused.contains("page two is on two canvases"), "{refused}");
    let refused = split_of(&format!(
        "{}{}",
        canvas(ONE, "one", "\"one\", \"four\""),
        canvas(TWO, "two", "\"two\", \"three\"")
    ));
    assert!(
        refused.contains("page four, which canvas.json does not list"),
        "{refused}"
    );
    let refused = split_of(&format!(
        "{}{}",
        canvas(ONE, "two", "\"one\""),
        canvas(TWO, "two", "\"two\", \"three\"")
    ));
    assert!(
        refused.contains(&format!(
            "{ONE} launches on page two, which is not one of its pages"
        )),
        "{refused}"
    );
}

#[test]
fn a_board_that_mounts_a_board_of_another_canvas_is_refused() {
    let mut files = boards();
    files.insert(
        "b.dc.html".to_string(),
        "<x-dc>B <dc-import name=\"a\"></dc-import></x-dc>".to_string(),
    );
    let refused = message(split(&index(), &parsed(&canvases_toml()), &files));
    assert!(
        refused.contains(&format!(
            "b.dc.html on {TWO} mounts a.dc.html, which is on {ONE}"
        )),
        "{refused}"
    );
}

#[test]
fn boards_and_the_index_must_name_the_same_files() {
    let mut files = boards();
    files.insert("e.dc.html".to_string(), "<x-dc>E</x-dc>".to_string());
    let refused = message(split(&index(), &parsed(&canvases_toml()), &files));
    assert!(
        refused.contains("e.dc.html is a board file that canvas.json does not place"),
        "{refused}"
    );
    let mut files = boards();
    files.remove("d.dc.html");
    let refused = message(split(&index(), &parsed(&canvases_toml()), &files));
    assert!(
        refused.contains("canvas.json places d.dc.html, which is no board file"),
        "{refused}"
    );
}

#[test]
fn a_canvas_over_the_file_limit_is_refused() {
    let mut index = index();
    let mut files = boards();
    for n in 0..MAX_FILES {
        let name = format!("many-{n}.dc.html");
        index["boards"][&name] = json!({"x": 0, "y": 0, "w": 10, "h": 10, "page": "two"});
        files.insert(name, "<x-dc>M</x-dc>".to_string());
    }
    let refused = message(split(&index, &parsed(&canvases_toml()), &files));
    assert!(
        refused.contains(&format!("{TWO} would hold")) && refused.contains("at most 512"),
        "{refused}"
    );
}

#[test]
fn the_canvases_file_names_each_canvas_by_its_claude_link() {
    let refused = message(parse_canvases(
        "[[canvas]]\nurl = \"https://example.com/x\"\ntitle = \"t\"\nlaunch = \"one\"\npages = [\"one\"]\n",
        CANVASES,
    ));
    assert!(refused.contains("https://claude.ai/artifact/"), "{refused}");
    let refused = message(parse_canvases("[[canvas]]\nurl = \"x\"\n", CANVASES));
    assert!(refused.contains(CANVASES), "{refused}");
    assert!(message(parse_canvases("", CANVASES)).contains("names no canvas"));
}

/// The index, then the design system, goes in the first batch, and no batch
/// holds more files than one publish sends.
#[test]
fn batches_hold_at_most_one_publishs_files_with_the_index_first() {
    let mut paths: Vec<String> = (0..300)
        .map(|n| format!("project/b{n:03}.dc.html"))
        .collect();
    paths.push("project/ds/demo/tokens.json".to_string());
    paths.push("project/canvas.json".to_string());
    let batches = batches(paths);
    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        [BATCH, 302 - BATCH]
    );
    assert_eq!(batches[0][0], "project/canvas.json");
    assert_eq!(batches[0][1], "project/ds/demo/tokens.json");
}
