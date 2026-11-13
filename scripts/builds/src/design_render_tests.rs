use super::*;
use crate::lists::fixtures::{design, lists};
use crate::lists::view::design_view;
use crate::lists::design::{Board, ReviewSection, plant_board, plant_review, plant_worktree};
use crate::testing::Scratch;

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().text()
}

/// A worktree with the design system's generated files, a board `A` that
/// mounts a board `B`, and a value its script gives.
fn tree(scratch: &Scratch) -> PathBuf {
    let dir = scratch.dir.join("wt");
    let mocks = dir.join("design/mocks");
    std::fs::create_dir_all(&mocks).unwrap();
    std::fs::create_dir_all(dir.join("design/system/components")).unwrap();
    std::fs::write(dir.join("design/system/tokens.css"), ":root { --tokens: 1; }").unwrap();
    std::fs::write(dir.join("design/system/components/bundle.css"), ".bundle { color: red; }").unwrap();
    std::fs::write(dir.join("design/system/components/bundle.js"), "window.Bae = { BUNDLE_MARK: 1 };").unwrap();
    std::fs::write(
        mocks.join("A.dc.html"),
        r#"<x-dc><helmet><style>.a-only { margin: 0 }</style></helmet>
<div class="a-only">{{greeting}} <dc-import name="B" word="{{greeting}}"></dc-import></div></x-dc>
<script type="text/x-dc" data-note="x">class Component extends DCLogic { renderVals() { return { greeting: "hello from A" }; } }</script>"#,
    )
    .unwrap();
    std::fs::write(mocks.join("B.dc.html"), "<x-dc><helmet><style>.b-only { top: 0 }</style></helmet><p class=\"b-only\">{{word}}</p></x-dc>")
        .unwrap();
    std::fs::write(mocks.join("canvas.json"), r#"{"v": 3, "boards": {"A.dc.html": {"w": 400, "h": 300}, "B.dc.html": {"w": 10}}}"#)
        .unwrap();
    dir
}

#[test]
fn a_board_renders_with_its_imported_board_its_values_and_the_design_system() {
    let scratch = Scratch::new("render-board");
    let dir = tree(&scratch);
    let page = render_board(&dir, "A", Theme::Light).unwrap();
    assert!(page.contains("<html data-theme=\"light\">"));
    for part in [
        "--tokens: 1",
        ".bundle { color: red; }",
        "BUNDLE_MARK",
        ".a-only { margin: 0 }",
        ".b-only { top: 0 }",
        "<template id=\"t-__main__\">",
        "<template id=\"t-B\">",
        "renderVals() { return { greeting: \"hello from A\" }; }",
        "C[\"B\"] = (function () { class Component extends DCLogic { renderVals() { return { yes: true }; } }",
    ] {
        assert!(page.contains(part), "the page has {part}");
    }
    assert!(!page.contains("<helmet>") && !page.contains("@@"), "helmets are cut out and every marker is filled");
    // The board by its path, and in the dark.
    let by_path = render_board(&dir, "design/mocks/A.dc.html", Theme::Dark).unwrap();
    assert!(by_path.contains("<html data-theme=\"dark\">"));
}

#[test]
fn a_board_that_cannot_be_drawn_says_what_is_missing() {
    let scratch = Scratch::new("render-board-missing");
    let dir = tree(&scratch);
    assert!(message(render_board(&dir, "Nope", Theme::Dark)).contains("Nope.dc.html is not there"));
    assert!(message(render_board(&dir, "../A", Theme::Dark)).contains("is no board"));
    std::fs::write(dir.join("design/mocks/C.dc.html"), "<x-dc><dc-import name=\"Gone\"></dc-import></x-dc>").unwrap();
    assert!(message(render_board(&dir, "C", Theme::Dark)).contains("mounts the board Gone, which is no file"));
    std::fs::write(dir.join("design/mocks/D.dc.html"), "no markup").unwrap();
    assert!(message(render_board(&dir, "D", Theme::Dark)).contains("has no <x-dc>"));
    std::fs::remove_file(dir.join("design/system/components/bundle.js")).unwrap();
    assert!(message(render_board(&dir, "A", Theme::Dark)).contains("bundle.js is not there"));
}

#[test]
fn a_part_holding_a_marker_is_not_filled_again() {
    let out = fill("<@@A@@><@@B@@>@@C@@ @@", &[("A", "@@B@@"), ("B", "b")]);
    assert_eq!(out, "<@@B@@><b>@@C@@ @@");
}

fn review_of(dir: &Path) -> (DesignChange, DesignView) {
    let mut state = lists();
    let id = design(&mut state);
    plant_worktree(&mut state, id, &dir.display().to_string());
    plant_board(&mut state, id, "design/mocks/A.dc.html", Board { link: "https://canvas.example/#A".into() });
    plant_review(&mut state, id, vec![
        ReviewSection {
            heading: "A </script> heading".into(),
            words: "words".into(),
            boards: vec![
                ReviewBoard { board: "design/mocks/A.dc.html".into(), caption: "the A".into(), theme: Theme::Dark },
                ReviewBoard { board: "design/mocks/B.dc.html".into(), caption: "no size".into(), theme: Theme::Light },
                ReviewBoard { board: "design/mocks/Z.dc.html".into(), caption: "no file".into(), theme: Theme::Light },
            ],
        },
        ReviewSection { heading: "Second".into(), words: String::new(), boards: vec![] },
    ]);
    let change = state.design(id).unwrap().clone();
    let view = design_view(&state, &change, None);
    (change, view)
}

fn page_data(page: &str) -> Value {
    let text = between(page, "<script type=\"application/json\" id=\"data\">", "</script>").expect("the data");
    serde_json::from_str(text).unwrap()
}

#[test]
fn a_review_page_has_its_sections_and_each_board_sized_from_the_canvas() {
    let scratch = Scratch::new("render-review");
    let dir = tree(&scratch);
    let (change, view) = review_of(&dir);
    let page = review_page(&change, &view).unwrap();
    assert!(!page.contains("A </script> heading"), "text can't end the data script");
    let data = page_data(&page);
    assert_eq!(data["sections"][0]["heading"], json!("A </script> heading"));
    assert_eq!(data["sections"][1]["boards"], json!([]));
    let boards = &data["sections"][0]["boards"];
    assert_eq!(
        (&boards[0]["w"], &boards[0]["h"], &boards[0]["src"], &boards[0]["theme"], &boards[0]["link"]),
        (&json!(400), &json!(300), &json!(format!("/design/{}/board/0", change.id.0)), &json!("dark"), &json!("https://canvas.example/#A"))
    );
    assert!(boards[1]["error"].as_str().unwrap().contains("no width and height for B.dc.html"), "{}", boards[1]);
    assert!(boards[2]["error"].as_str().unwrap().contains("no width and height for Z.dc.html"), "{}", boards[2]);
    assert_eq!(boards[1]["src"], Value::Null);
    // The board a frame loads is the one the page counts as its <n>th.
    assert_eq!(review_board(&change, 1).unwrap().caption, "no size");
    assert!(review_board(&change, 3).is_none());
}

#[test]
fn a_review_page_of_a_worktree_with_no_canvas_file_says_so_at_each_board() {
    let scratch = Scratch::new("render-review-nocanvas");
    let dir = tree(&scratch);
    std::fs::remove_file(dir.join("design/mocks/canvas.json")).unwrap();
    let (change, view) = review_of(&dir);
    let data = page_data(&review_page(&change, &view).unwrap());
    assert!(data["sections"][0]["boards"][0]["error"].as_str().unwrap().contains("canvas.json is not there"));
}
