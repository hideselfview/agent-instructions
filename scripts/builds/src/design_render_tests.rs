use super::*;
use crate::lists::design::ReviewSection;
use crate::lists::fixtures::{db, design_drawn, design_tipped, read};
use crate::lists::view::design_view;
use crate::testing::{Scratch, git, repo};
use std::path::PathBuf;

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().text()
}

/// A worktree with the design system's generated files, a board `A` that
/// mounts a board `B`, and a value its script gives. The files are written,
/// not committed.
fn tree(scratch: &Scratch) -> PathBuf {
    let dir = scratch.dir.join("wt");
    repo(&dir, "");
    let mocks = dir.join("design/mocks");
    std::fs::create_dir_all(&mocks).unwrap();
    std::fs::create_dir_all(dir.join("design/system/components")).unwrap();
    std::fs::write(
        dir.join("design/system/tokens.css"),
        ":root { --tokens: 1; }",
    )
    .unwrap();
    std::fs::write(
        dir.join("design/system/components/bundle.css"),
        ".bundle { color: red; }",
    )
    .unwrap();
    std::fs::write(
        dir.join("design/system/components/bundle.js"),
        "window.Bae = { BUNDLE_MARK: 1 };",
    )
    .unwrap();
    std::fs::write(
        mocks.join("A.dc.html"),
        r#"<x-dc><helmet><style>.a-only { margin: 0 }</style></helmet>
<div class="a-only">{{greeting}} <dc-import name="B" word="{{greeting}}"></dc-import></div></x-dc>
<script type="text/x-dc" data-note="x">class Component extends DCLogic { renderVals() { return { greeting: "hello from A" }; } }</script>"#,
    )
    .unwrap();
    std::fs::write(mocks.join("B.dc.html"), "<x-dc><helmet><style>.b-only { top: 0 }</style></helmet><p class=\"b-only\">{{word}}</p></x-dc>")
        .unwrap();
    std::fs::write(
        mocks.join("canvas.json"),
        r#"{"v": 3, "boards": {"A.dc.html": {"w": 400, "h": 300}, "B.dc.html": {"w": 10}}}"#,
    )
    .unwrap();
    dir
}

/// Commits everything in <dir> and returns the commit.
fn commit(dir: &Path) -> String {
    git(dir, &["add", "design"]);
    git(dir, &["commit", "-q", "-m", "boards"]);
    crate::lists::design_paths::full_sha(dir, "HEAD").unwrap()
}

#[test]
fn a_board_renders_with_its_imported_board_its_values_and_the_design_system() {
    let scratch = Scratch::new("render-board");
    let dir = tree(&scratch);
    let page = render_board(Source::Dir(&dir), "A", Theme::Light).unwrap();
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
    assert!(
        !page.contains("<helmet>") && !page.contains("@@"),
        "helmets are cut out and every marker is filled"
    );
    // The board by its path, and in the dark.
    let by_path = render_board(Source::Dir(&dir), "design/mocks/A.dc.html", Theme::Dark).unwrap();
    assert!(by_path.contains("<html data-theme=\"dark\">"));
}

#[test]
fn a_board_that_cannot_be_drawn_says_what_is_missing() {
    let scratch = Scratch::new("render-board-missing");
    let dir = tree(&scratch);
    assert!(
        message(render_board(Source::Dir(&dir), "Nope", Theme::Dark))
            .contains("Nope.dc.html is not there")
    );
    assert!(message(render_board(Source::Dir(&dir), "../A", Theme::Dark)).contains("is no board"));
    std::fs::write(
        dir.join("design/mocks/C.dc.html"),
        "<x-dc><dc-import name=\"Gone\"></dc-import></x-dc>",
    )
    .unwrap();
    assert!(
        message(render_board(Source::Dir(&dir), "C", Theme::Dark))
            .contains("mounts the board Gone, which is no file")
    );
    std::fs::write(dir.join("design/mocks/D.dc.html"), "no markup").unwrap();
    assert!(message(render_board(Source::Dir(&dir), "D", Theme::Dark)).contains("has no <x-dc>"));
    std::fs::remove_file(dir.join("design/system/components/bundle.js")).unwrap();
    assert!(
        message(render_board(Source::Dir(&dir), "A", Theme::Dark))
            .contains("bundle.js is not there")
    );
}

#[test]
fn a_part_holding_a_marker_is_not_filled_again() {
    let out = fill("<@@A@@><@@B@@>@@C@@ @@", &[("A", "@@B@@"), ("B", "b")]);
    assert_eq!(out, "<@@B@@><b>@@C@@ @@");
}

/// The review page of a change drawn in <dir> whose last commit is <tip>.
fn review_of(dir: &Path, tip: &str) -> (DesignChange, DesignView) {
    let db = db();
    let id = design_drawn(
        &db,
        &dir.display().to_string(),
        &[("design/mocks/A.dc.html", "https://canvas.example/#A")],
        vec![
            ReviewSection {
                heading: "A </script> heading".into(),
                words: "words".into(),
                boards: vec![
                    ReviewBoard {
                        board: "design/mocks/A.dc.html".into(),
                        caption: "the A".into(),
                        theme: Theme::Dark,
                    },
                    ReviewBoard {
                        board: "design/mocks/B.dc.html".into(),
                        caption: "no size".into(),
                        theme: Theme::Light,
                    },
                    ReviewBoard {
                        board: "design/mocks/Z.dc.html".into(),
                        caption: "no file".into(),
                        theme: Theme::Light,
                    },
                ],
            },
            ReviewSection {
                heading: "Second".into(),
                words: String::new(),
                boards: vec![],
            },
        ],
    );
    design_tipped(&db, id, tip);
    let state = read(&db);
    let change = state.design(id).unwrap().clone();
    let view = design_view(&state, &change, None);
    (change, view)
}

fn page_data(page: &str) -> Value {
    let text = between(
        page,
        "<script type=\"application/json\" id=\"data\">",
        "</script>",
    )
    .expect("the data");
    serde_json::from_str(text).unwrap()
}

#[test]
fn a_review_page_has_its_sections_and_each_board_sized_from_the_canvas() {
    let scratch = Scratch::new("render-review");
    let dir = tree(&scratch);
    let tip = commit(&dir);
    let (change, view) = review_of(&dir, &tip);
    let page = review_page(&change, &view).unwrap();
    assert!(
        !page.contains("A </script> heading"),
        "text can't end the data script"
    );
    let data = page_data(&page);
    assert_eq!(data["sections"][0]["heading"], json!("A </script> heading"));
    assert_eq!(data["sections"][1]["boards"], json!([]));
    let boards = &data["sections"][0]["boards"];
    assert_eq!(
        (
            &boards[0]["w"],
            &boards[0]["h"],
            &boards[0]["src"],
            &boards[0]["theme"],
            &boards[0]["link"]
        ),
        (
            &json!(400),
            &json!(300),
            &json!(format!("/design/{}/board/0", change.id.0)),
            &json!("dark"),
            &json!("https://canvas.example/#A")
        )
    );
    assert!(
        boards[1]["error"]
            .as_str()
            .unwrap()
            .contains("no width and height for B.dc.html"),
        "{}",
        boards[1]
    );
    assert!(
        boards[2]["error"]
            .as_str()
            .unwrap()
            .contains("no width and height for Z.dc.html"),
        "{}",
        boards[2]
    );
    assert_eq!(boards[1]["src"], Value::Null);
    // The board a frame loads is the one the page counts as its <n>th.
    assert_eq!(review_board(&change, 1).unwrap().caption, "no size");
    assert!(review_board(&change, 3).is_none());
}

#[test]
fn a_review_page_names_the_boards_the_change_deletes_without_drawing_them() {
    let scratch = Scratch::new("render-review-deleted");
    let dir = tree(&scratch);
    let tip = commit(&dir);
    let (mut change, _) = review_of(&dir, &tip);
    change.deleted.insert("design/mocks/Old.dc.html".into());
    let view = design_view(&crate::lists::fixtures::read(&db()), &change, None);
    let data = page_data(&review_page(&change, &view).unwrap());
    assert_eq!(data["deleted"], json!(["design/mocks/Old.dc.html"]));
}

#[test]
fn a_review_page_of_a_commit_with_no_canvas_file_says_so_at_each_board() {
    let scratch = Scratch::new("render-review-nocanvas");
    let dir = tree(&scratch);
    std::fs::remove_file(dir.join("design/mocks/canvas.json")).unwrap();
    let tip = commit(&dir);
    let (change, view) = review_of(&dir, &tip);
    let data = page_data(&review_page(&change, &view).unwrap());
    assert!(
        data["sections"][0]["boards"][0]["error"]
            .as_str()
            .unwrap()
            .contains("canvas.json is not there")
    );
}

#[test]
fn a_board_is_read_at_a_commit_whatever_the_worktree_has_checked_out() {
    let scratch = Scratch::new("render-commit");
    let dir = tree(&scratch);
    let first = commit(&dir);
    // The worktree moves on: another branch with another board, then edits that are no commit.
    git(&dir, &["checkout", "-q", "-b", "other"]);
    std::fs::write(
        dir.join("design/mocks/A.dc.html"),
        "<x-dc><p>board A of another branch</p></x-dc>",
    )
    .unwrap();
    std::fs::write(
        dir.join("design/system/components/bundle.js"),
        "window.Bae = { OTHER_MARK: 1 };",
    )
    .unwrap();
    let second = commit(&dir);
    std::fs::write(dir.join("design/mocks/A.dc.html"), "<x-dc>edited</x-dc>").unwrap();

    let at =
        |rev: &str| render_board(Source::Commit { repo: &dir, rev }, "A", Theme::Dark).unwrap();
    let old = at(&first);
    assert!(old.contains("hello from A") && old.contains("BUNDLE_MARK"));
    assert!(!old.contains("another branch") && !old.contains("OTHER_MARK"));
    let new = at(&second);
    assert!(new.contains("board A of another branch") && new.contains("OTHER_MARK"));
    assert!(!new.contains("edited"));
    // A branch name reads the same way, and a revision that is no commit is said so.
    assert!(at("other").contains("another branch"));
    let missing = message(render_board(
        Source::Commit {
            repo: &dir,
            rev: "nope",
        },
        "A",
        Theme::Dark,
    ));
    assert!(missing.contains("find the commit nope"), "{missing}");
    let absent = message(render_board(
        Source::Commit {
            repo: &dir,
            rev: &first,
        },
        "Nope",
        Theme::Dark,
    ));
    assert!(
        absent.contains(&format!("{first}:design/mocks/Nope.dc.html is not there")),
        "{absent}"
    );
}

#[test]
fn two_changes_in_one_worktree_each_draw_their_own_last_commit() {
    let scratch = Scratch::new("render-two-changes");
    let dir = tree(&scratch);
    let first = commit(&dir);
    git(&dir, &["checkout", "-q", "-b", "second"]);
    std::fs::write(
        dir.join("design/mocks/canvas.json"),
        r#"{"v": 3, "boards": {"A.dc.html": {"w": 900, "h": 700}, "B.dc.html": {"w": 10}}}"#,
    )
    .unwrap();
    let second = commit(&dir);
    // The worktree has the second change's branch checked out.
    let boards = |tip: &str| {
        let (change, view) = review_of(&dir, tip);
        let data = page_data(&review_page(&change, &view).unwrap());
        data["sections"][0]["boards"][0].clone()
    };
    let (one, two) = (boards(&first), boards(&second));
    assert_eq!((&one["w"], &one["h"]), (&json!(400), &json!(300)));
    assert_eq!((&two["w"], &two["h"]), (&json!(900), &json!(700)));
}

#[test]
fn a_review_page_of_a_change_with_no_last_commit_says_to_amend_it() {
    let scratch = Scratch::new("render-no-tip");
    let dir = tree(&scratch);
    commit(&dir);
    let (mut change, view) = review_of(&dir, "0000000");
    change.tip = None;
    let data = page_data(&review_page(&change, &view).unwrap());
    let error = data["sections"][0]["boards"][0]["error"].as_str().unwrap();
    assert!(error.contains("amend it"), "{error}");
}
