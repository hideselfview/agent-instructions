use super::*;
use crate::testing::{Scratch, design_system, git, repo};
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
fn the_page_has_each_page_of_the_index_with_its_boards_in_the_canvas_rows_at_origin_main() {
    let scratch = Scratch::new("designs-page");
    let (dir, commit) = designs_repo(&scratch);
    let page = page(
        "demo",
        &dir,
        &scratch.dir.join("cache"),
        Ok(Standing::default()),
    )
    .unwrap();
    assert!(!page.contains("@@"), "every marker is filled");
    let data = page_data(&page);
    assert_eq!(data["commit"], json!(commit));
    assert_eq!(data["boards"], json!(4));
    assert_eq!(data["base"], json!(format!("/designs/demo/{commit}")));
    assert_eq!(
        data["pages"],
        json!([
            { "id": "one", "name": "One", "boards": [
                { "name": "C", "title": null, "w": 100, "h": 100 },
                { "name": "D", "title": "No size",
                  "error": "design/mocks/canvas.json gives no width and height for D.dc.html" },
            ] },
            { "id": "two", "name": "Two", "boards": [
                { "name": "B", "title": "Left", "w": 412, "h": 916 },
                { "name": "A", "title": "Right", "w": 400, "h": 300 },
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
        json!([{ "name": "Row", "height": 120 }])
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
        )
        .unwrap(),
    );
    let error = data["system"]["error"].as_str().unwrap();
    assert!(error.contains("design/system/tokens.json"), "{error}");
    assert_eq!(data["components"][0]["name"], json!("Row"));
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
    let data = page_data(&page("demo", &dir, &cache, Ok(Standing::default())).unwrap());
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
    let data = page_data(&page("demo", &dir, &cache, Ok(Standing::default())).unwrap());
    assert_eq!(
        data["publish"],
        json!({
            "canvas": true,
            "standing": { "latest": null, "published": null },
        })
    );
    let unreadable = page(
        "demo",
        &dir,
        &cache,
        Err(crate::fail::Fail::Message("the store is gone".into())),
    )
    .unwrap();
    assert_eq!(
        page_data(&unreadable)["publish"],
        json!({ "error": "the store is gone" })
    );
}
