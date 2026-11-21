use super::*;
use crate::testing::{Scratch, design_system, git, repo, run_script};
use std::path::PathBuf;

/// A repo whose origin/main holds four boards on two pages, a design system
/// with one color, one text style and a component `Row`, and nothing else
/// at HEAD. Returns the repo and origin/main's commit.
fn designs_repo(scratch: &Scratch) -> (PathBuf, String) {
    let dir = scratch.dir.join("repo");
    repo(&dir, "");
    design_system(&dir, "1", ".bundle { color: red; }", "window.Demo = {};");
    let write = |rel: &str, text: &str| {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "design/system/tokens.json",
        &json!({
            "name": "demo",
            "color": { "themes": [{ "id": "dark" }, { "id": "light" }],
                "tokens": [{ "name": "bg", "value": { "dark": "#111111", "light": "#EEEEEE" } }] },
            "type": { "families": { "sans": "Inter, sans-serif" },
                "groups": [{ "family": "sans", "styles": [
                    { "name": "title", "fontSize": "20px", "fontWeight": 600, "sample": "Low Tide Hours" }] }] },
            "spacing": { "tokens": [{ "name": "tokens", "value": "1" }] },
        })
        .to_string(),
    );
    write(
        "design/system/components/Row/preview.html",
        "<!-- @dsCard group=\"Mac\" height=120 -->\n<!doctype html>\n<html><head><title>Row</title></head><body><script>draw()</script></body></html>",
    );
    write(
        "design/system/groups.json",
        &json!([{ "name": "Rows", "about": "The rows.", "components": ["Row"] }]).to_string(),
    );
    for name in ["A", "B", "C", "D"] {
        write(
            &format!("design/mocks/{name}.dc.html"),
            &format!("<x-dc><p>board {name}</p></x-dc>"),
        );
    }
    write(
        "design/mocks/canvas.json",
        &json!({
            "v": 3,
            "pages": [{ "id": "one", "name": "One" }, { "id": "two", "name": "Two" }],
            "boards": {
                "A.dc.html": { "page": "two", "title": "Right", "x": 500, "y": 0, "w": 400, "h": 300 },
                "B.dc.html": { "page": "two", "title": "Left", "x": 0, "y": 0, "w": 412, "h": 916 },
                "C.dc.html": { "x": 0, "y": 0, "w": 100, "h": 100 },
                "D.dc.html": { "page": "one", "title": "No size", "x": 0, "y": 900 },
                "E.dc.html": { "page": "one", "title": "No place", "w": 100, "h": 100 },
            },
        })
        .to_string(),
    );
    git(&dir, &["add", "design"]);
    git(&dir, &["commit", "-q", "-m", "designs"]);
    let commit = crate::lists::design_paths::full_sha(&dir, "HEAD").unwrap();
    git(&dir, &["update-ref", "refs/remotes/origin/main", &commit]);
    // What HEAD holds after origin/main is not drawn.
    git(&dir, &["rm", "-q", "-r", "design/mocks"]);
    git(&dir, &["commit", "-q", "-m", "gone"]);
    (dir, commit)
}

fn page_data(page: &str) -> Value {
    let start = page
        .find("<script type=\"application/json\" id=\"data\">")
        .expect("the data")
        + "<script type=\"application/json\" id=\"data\">".len();
    let end = start + page[start..].find("</script>").unwrap();
    serde_json::from_str(&page[start..end]).unwrap()
}

#[test]
fn the_page_has_each_page_of_the_index_with_its_boards_at_their_canvas_places_at_origin_main() {
    let scratch = Scratch::new("designs-page");
    let (dir, commit) = designs_repo(&scratch);
    let page = page(
        "demo",
        &dir,
        &scratch.dir.join("cache"),
        Ok(Standing::default()),
        &Value::Null,
    )
    .unwrap();
    assert!(!page.contains("@@"), "every marker is filled");
    let data = page_data(&page);
    assert_eq!(data["commit"], json!(commit));
    assert_eq!(data["base"], json!(format!("/designs/demo/{commit}")));
    assert_eq!(
        data["pages"],
        json!([
            { "id": "one", "name": "One", "boards": [
                { "name": "C", "title": null, "x": 0.0, "y": 0.0, "w": 100, "h": 100 },
                { "name": "D", "title": "No size",
                  "error": "design/mocks/canvas.json gives no width and height for D.dc.html" },
                { "name": "E", "title": "No place",
                  "error": "design/mocks/canvas.json gives no x and y for E.dc.html" },
            ] },
            { "id": "two", "name": "Two", "boards": [
                { "name": "A", "title": "Right", "x": 500.0, "y": 0.0, "w": 400, "h": 300 },
                { "name": "B", "title": "Left", "x": 0.0, "y": 0.0, "w": 412, "h": 916 },
            ] },
        ])
    );
    assert_eq!(
        data["system"],
        json!({
            "colors": [{ "name": "bg", "value": "#111111" }],
            "styles": [{ "name": "title", "css": "font-family: Inter, sans-serif; font-size: 20px; font-weight: 600",
                "words": "Low Tide Hours" }],
        })
    );
    assert_eq!(
        data["components"],
        json!({
            "groups": [{ "name": "Rows", "about": "The rows.",
                "cards": [{ "name": "Row", "height": 120 }], "subgroups": [] }],
            "problems": [],
        })
    );
}

#[test]
fn a_design_system_that_cannot_be_read_shows_its_problem_and_a_bad_index_fails_the_page() {
    let scratch = Scratch::new("designs-page-problems");
    let (dir, commit) = designs_repo(&scratch);
    git(&dir, &["checkout", "-q", &commit]);
    std::fs::write(dir.join("design/system/tokens.json"), "{").unwrap();
    std::fs::write(dir.join("design/mocks/canvas.json"), "{\"boards\": {}}").unwrap();
    git(&dir, &["commit", "-q", "-am", "broken"]);
    let broken = crate::lists::design_paths::full_sha(&dir, "HEAD").unwrap();
    git(&dir, &["update-ref", "refs/remotes/origin/main", &broken]);
    let error = page(
        "demo",
        &dir,
        &scratch.dir.join("cache"),
        Ok(Standing::default()),
        &Value::Null,
    )
    .unwrap_err()
    .text();
    assert!(error.contains("canvas.json lists no pages"), "{error}");
    git(
        &dir,
        &["checkout", "-q", &commit, "--", "design/mocks/canvas.json"],
    );
    git(&dir, &["commit", "-q", "-m", "index back"]);
    let fixed = crate::lists::design_paths::full_sha(&dir, "HEAD").unwrap();
    git(&dir, &["update-ref", "refs/remotes/origin/main", &fixed]);
    let data = page_data(
        &page(
            "demo",
            &dir,
            &scratch.dir.join("cache"),
            Ok(Standing::default()),
            &Value::Null,
        )
        .unwrap(),
    );
    let error = data["system"]["error"].as_str().unwrap();
    assert!(error.contains("design/system/tokens.json"), "{error}");
    assert_eq!(
        data["components"]["groups"][0]["cards"][0]["name"],
        json!("Row")
    );
}

#[test]
fn a_frame_draws_its_board_or_its_preview_at_the_commit_the_page_names() {
    let scratch = Scratch::new("designs-frames");
    let (dir, commit) = designs_repo(&scratch);
    let cache = scratch.dir.join("cache");
    let drawn = board(&dir, &commit, "A", &cache).unwrap();
    assert!(drawn.contains("<html data-theme=\"dark\">") && drawn.contains("board A"));
    let error = board(&dir, "HEAD", "A", &cache).unwrap_err().text();
    assert!(error.contains("is not there"), "{error}");
    let card = preview(&dir, &commit, "Row", &cache).unwrap();
    let order: Vec<usize> = [
        "<head>",
        "previewWidth",
        "--tokens: 1",
        ".bundle { color: red; }",
        "react.production.min.js",
        "window.Demo",
        "<title>Row</title>",
        "draw()",
    ]
    .iter()
    .map(|part| {
        card.find(part)
            .unwrap_or_else(|| panic!("{part} in {card}"))
    })
    .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    let error = preview(&dir, &commit, "../Row", &cache).unwrap_err().text();
    assert!(error.contains("is no component"), "{error}");
}

#[test]
fn a_commit_is_forty_hex_digits() {
    assert!(is_commit(&"a1".repeat(20)));
    assert!(!is_commit("HEAD"));
    assert!(!is_commit(&"g1".repeat(20)));
    assert!(!is_commit(&"a1".repeat(19)));
}

#[test]
fn the_page_offers_publishing_only_where_canvases_toml_names_the_artifacts() {
    let scratch = Scratch::new("designs-publishing");
    let (dir, _) = designs_repo(&scratch);
    let cache = scratch.dir.join("cache");
    let data =
        page_data(&page("demo", &dir, &cache, Ok(Standing::default()), &Value::Null).unwrap());
    assert_eq!(data["publish"], json!({ "canvas": false }));
    git(&dir, &["checkout", "-q", "origin/main"]);
    std::fs::write(
        dir.join(CANVASES),
        "[[canvas]]\ntitle = \"demo boards\"\nlaunch = \"one\"\npages = [\"one\", \"two\"]\n\n[design_system]\ntitle = \"demo design system\"\n",
    )
    .unwrap();
    git(&dir, &["add", CANVASES]);
    git(&dir, &["commit", "-q", "-m", "canvases"]);
    let commit = crate::lists::design_paths::full_sha(&dir, "HEAD").unwrap();
    git(&dir, &["update-ref", "refs/remotes/origin/main", &commit]);
    let data =
        page_data(&page("demo", &dir, &cache, Ok(Standing::default()), &Value::Null).unwrap());
    assert_eq!(
        data["publish"],
        json!({
            "canvas": true,
            "standing": { "latest": null },
        })
    );
    let unreadable = page(
        "demo",
        &dir,
        &cache,
        Err(crate::fail::Fail::Message("the store is gone".into())),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(
        page_data(&unreadable)["publish"],
        json!({ "error": "the store is gone" })
    );
}

#[test]
fn the_system_tab_gets_the_groups_and_subgroups_in_the_order_groups_json_lists_them() {
    let scratch = Scratch::new("designs-groups");
    let (dir, _) = designs_repo(&scratch);
    git(&dir, &["checkout", "-q", "origin/main"]);
    let preview = |name: &str, height: u64| {
        let path = dir.join(format!("design/system/components/{name}/preview.html"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("<!-- @dsCard group=\"Mac\" height={height} -->\n<!doctype html>"),
        )
        .unwrap();
    };
    preview("Button", 60);
    preview("Avatar", 90);
    preview("Orphan", 30);
    std::fs::write(
        dir.join("design/system/groups.json"),
        json!([
            { "name": "Zeta", "about": "Listed first.", "components": ["Row"] },
            { "name": "Alpha", "about": "Listed second.", "subgroups": [
                { "name": "Second", "about": "Listed first in Alpha.", "components": ["Button"] },
                { "name": "First", "about": "Listed last in Alpha.", "components": ["Avatar"] },
            ] },
        ])
        .to_string(),
    )
    .unwrap();
    git(&dir, &["add", "design"]);
    git(&dir, &["commit", "-q", "-m", "groups"]);
    let commit = crate::lists::design_paths::full_sha(&dir, "HEAD").unwrap();
    git(&dir, &["update-ref", "refs/remotes/origin/main", &commit]);
    let data = page_data(
        &page(
            "demo",
            &dir,
            &scratch.dir.join("cache"),
            Ok(Standing::default()),
            &Value::Null,
        )
        .unwrap(),
    );
    assert_eq!(
        data["components"],
        json!({
            "groups": [
                { "name": "Zeta", "about": "Listed first.",
                  "cards": [{ "name": "Row", "height": 120 }], "subgroups": [] },
                { "name": "Alpha", "about": "Listed second.", "cards": [], "subgroups": [
                    { "name": "Second", "about": "Listed first in Alpha.",
                      "cards": [{ "name": "Button", "height": 60 }] },
                    { "name": "First", "about": "Listed last in Alpha.",
                      "cards": [{ "name": "Avatar", "height": 90 }] },
                ] },
            ],
            "problems": ["design/system/groups.json lists no group for Orphan."],
        })
    );
}

/// The nodes of a page's design system tab as the filter's script reads
/// them: each has classes, an optional `data-name`, and children.
const FAKE_DOM: &str = r#"
class Node {
  constructor(classes, name, children = []) {
    this.classes = classes; this.dataset = name === undefined ? {} : {name};
    this.hidden = false; this.children = children; this.attrs = {};
  }
  all() { return this.children.flatMap(c => [c, ...c.all()]); }
  querySelectorAll(selector) {
    if (selector === "[data-none]") return this.all().filter(n => "none" in n.attrs);
    if (selector === "[data-name]") return this.all().filter(n => "name" in n.dataset);
    const wanted = selector.split(",").map(s => s.trim().slice(1));
    return this.all().filter(n => n.classes.some(c => wanted.includes(c)));
  }
  remove() { for (const n of [this, ...this.all()]) n.removed = true; }
  append(node) { this.children.push(node); }
}
const item = name => new Node(["ds-comp"], name);
const rows = [item("Row"), item("Button"), item("Avatar")];
const sub = new Node(["ds-sub"], undefined, [new Node(["ds-cards"], undefined, [rows[1]])]);
const first = new Node(["ds-group"], undefined, [new Node(["ds-cards"], undefined, [rows[0]])]);
const second = new Node(["ds-group"], undefined, [sub, new Node(["ds-cards"], undefined, [rows[2]])]);
const main = new Node(["ds-sec"], undefined, [first, second]);
const none = () => { const n = new Node(["dz-sub"]); n.attrs.none = ""; return n; };
const shownNames = () => rows.filter(r => !r.hidden).map(r => r.dataset.name);
const groupsShown = () => [first, second, sub].map(p => !p.hidden);
"#;

fn filter_run(steps: &str) -> Value {
    let script = format!(
        "{}\n{FAKE_DOM}\nconst out = [];\n{steps}",
        include_str!("designs_filter.js")
    );
    run_script(&script, "out")
}

#[test]
fn the_filter_keeps_the_components_whose_names_hold_the_text_and_hides_empty_groups() {
    let steps = r#"
      filterParts(main, "", none);
      out.push([shownNames(), groupsShown()]);
      filterParts(main, "butt", none);
      out.push([shownNames(), groupsShown()]);
      filterParts(main, "row", none);
      out.push([shownNames(), groupsShown()]);
      filterParts(main, "", none);
      out.push([shownNames(), groupsShown()]);
    "#;
    assert_eq!(
        filter_run(steps),
        json!([
            [["Row", "Button", "Avatar"], [true, true, true]],
            [["Button"], [false, true, true]],
            [["Row"], [true, false, false]],
            [["Row", "Button", "Avatar"], [true, true, true]],
        ])
    );
}

#[test]
fn the_filter_says_so_when_no_component_matches_and_stops_saying_it_when_one_does() {
    let steps = r#"
      filterParts(main, "zzz", none);
      out.push([shownNames(), main.children.filter(c => "none" in c.attrs).length]);
      filterParts(main, "zzz", none);
      out.push(main.children.filter(c => "none" in c.attrs && !c.removed).length);
      filterParts(main, "avatar", none);
      out.push([shownNames(), main.children.filter(c => "none" in c.attrs && !c.removed).length]);
    "#;
    assert_eq!(filter_run(steps), json!([[[], 1], 1, [["Avatar"], 0]]));
}

#[test]
fn the_page_carries_the_filter_script_the_tests_run() {
    let scratch = Scratch::new("designs-filter-page");
    let (dir, _) = designs_repo(&scratch);
    let page = page(
        "demo",
        &dir,
        &scratch.dir.join("cache"),
        Ok(Standing::default()),
        &Value::Null,
    )
    .unwrap();
    assert!(page.contains(include_str!("designs_filter.js")));
}
