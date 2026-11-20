use super::*;
use crate::lists::design::{Around, Circle, ReviewSection};
use crate::lists::fixtures::{db, design_drawn, design_tipped, read};
use crate::lists::view::design_view;
use crate::testing::{Scratch, design_system, git, repo};
use std::path::PathBuf;

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().text()
}

/// Where a test keeps the design systems it generates for commits.
fn cache(scratch: &Scratch) -> PathBuf {
    scratch.dir.join("design-systems")
}

/// A worktree with a board `A` that mounts a board `B` and a value its
/// script gives, and a design system as the bae repo holds one: its sources
/// and the build-bundle.py that builds the bundles from them, with the
/// bundles ignored by git. The bundles are on disk too, as a worktree that
/// ran the script has them. Nothing is committed.
fn tree(scratch: &Scratch) -> PathBuf {
    let dir = scratch.dir.join("wt");
    repo(&dir, "");
    let mocks = dir.join("design/mocks");
    std::fs::create_dir_all(&mocks).unwrap();
    design_system(
        &dir,
        "1",
        ".bundle { color: red; }",
        "window.Bae = { BUNDLE_MARK: 1 };",
    );
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

/// Commits everything in <dir> that git does not ignore and returns the
/// commit.
fn commit(dir: &Path) -> String {
    git(dir, &["add", "design"]);
    git(dir, &["commit", "-q", "-m", "boards"]);
    crate::lists::design_paths::full_sha(dir, "HEAD").unwrap()
}

#[test]
fn a_board_renders_with_its_imported_board_its_values_and_the_design_system() {
    let scratch = Scratch::new("render-board");
    let dir = tree(&scratch);
    let page = render_board(
        Source::Dir(&dir),
        "A",
        Theme::Light,
        &Props::new(),
        &cache(&scratch),
    )
    .unwrap();
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
    let by_path = render_board(
        Source::Dir(&dir),
        "design/mocks/A.dc.html",
        Theme::Dark,
        &Props::new(),
        &cache(&scratch),
    )
    .unwrap();
    assert!(by_path.contains("<html data-theme=\"dark\">"));
}

#[test]
fn a_board_that_cannot_be_drawn_says_what_is_missing() {
    let scratch = Scratch::new("render-board-missing");
    let dir = tree(&scratch);
    assert!(
        message(render_board(
            Source::Dir(&dir),
            "Nope",
            Theme::Dark,
            &Props::new(),
            &cache(&scratch)
        ))
        .contains("Nope.dc.html is not there")
    );
    assert!(
        message(render_board(
            Source::Dir(&dir),
            "../A",
            Theme::Dark,
            &Props::new(),
            &cache(&scratch)
        ))
        .contains("is no board")
    );
    std::fs::write(
        dir.join("design/mocks/C.dc.html"),
        "<x-dc><dc-import name=\"Gone\"></dc-import></x-dc>",
    )
    .unwrap();
    assert!(
        message(render_board(
            Source::Dir(&dir),
            "C",
            Theme::Dark,
            &Props::new(),
            &cache(&scratch)
        ))
        .contains("mounts the board Gone, which is no file")
    );
    std::fs::write(dir.join("design/mocks/D.dc.html"), "no markup").unwrap();
    assert!(
        message(render_board(
            Source::Dir(&dir),
            "D",
            Theme::Dark,
            &Props::new(),
            &cache(&scratch)
        ))
        .contains("has no <x-dc>")
    );
    std::fs::write(
        dir.join("design/system/build-bundle.py"),
        "raise SystemExit('no bundles')",
    )
    .unwrap();
    assert!(
        message(render_board(
            Source::Dir(&dir),
            "A",
            Theme::Dark,
            &Props::new(),
            &cache(&scratch)
        ))
        .contains("build-bundle.py failed")
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
        Some(ReviewSection {
            heading: "A </script> heading".into(),
            words: "words".into(),
            boards: vec![
                ReviewBoard {
                    board: "design/mocks/A.dc.html".into(),
                    caption: "the A".into(),
                    theme: Theme::Dark,
                    circles: vec![],
                },
                ReviewBoard {
                    board: "design/mocks/B.dc.html".into(),
                    caption: "no size".into(),
                    theme: Theme::Light,
                    circles: vec![],
                },
                ReviewBoard {
                    board: "design/mocks/Z.dc.html".into(),
                    caption: "no file".into(),
                    theme: Theme::Light,
                    circles: vec![],
                },
            ],
        }),
    );
    design_tipped(&db, id, tip);
    let state = read(&db);
    let change = state.design(id).unwrap().clone();
    let view = design_view(&state, &change, None).unwrap();
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

/// Makes <commit> what origin/main is at.
fn set_origin_main(dir: &Path, commit: &str) {
    git(dir, &["update-ref", "refs/remotes/origin/main", commit]);
}

#[test]
fn a_review_page_has_its_heading_and_each_board_now_and_proposed_sized_from_the_canvas() {
    let scratch = Scratch::new("render-review");
    let dir = tree(&scratch);
    let tip = commit(&dir);
    set_origin_main(&dir, &tip);
    let (change, view) = review_of(&dir, &tip);
    let page = review_page(&change, &view, "demo").unwrap();
    assert!(
        !page.contains("A </script> heading"),
        "text can't end the data script"
    );
    let data = page_data(&page);
    assert_eq!(data["section"]["heading"], json!("A </script> heading"));
    let boards = &data["section"]["boards"];
    let id = change.id.0;
    for side in ["now", "proposed"] {
        assert_eq!(
            boards[0][side],
            json!({ "w": 400, "h": 300, "src": format!("/design/demo/{id}/board/0/{side}") }),
            "{side}"
        );
    }
    assert_eq!(
        (&boards[0]["theme"], &boards[0]["link"]),
        (&json!("dark"), &json!("https://canvas.example/#A"))
    );
    for side in ["now", "proposed"] {
        let error = boards[1][side]["error"].as_str().unwrap();
        assert!(
            error.contains("no width and height for B.dc.html"),
            "{error}"
        );
        assert_eq!(boards[1][side]["src"], Value::Null);
    }
    // Z is no file anywhere: the change's own drawing of it is a problem, origin/main's is an absence.
    let error = boards[2]["proposed"]["error"].as_str().unwrap();
    assert!(error.contains("Z.dc.html is no file"), "{error}");
    assert_eq!(boards[2]["now"], json!({ "absent": true }));
    // The board a frame loads is the one the page counts as its <n>th.
    assert_eq!(review_board(&change, 1).unwrap().caption, "no size");
    assert!(review_board(&change, 3).is_none());
}

#[test]
fn now_is_origin_main_and_proposed_is_the_last_commit_each_with_its_own_design_system() {
    let scratch = Scratch::new("render-now-proposed");
    let dir = tree(&scratch);
    let main = commit(&dir);
    set_origin_main(&dir, &main);
    git(&dir, &["checkout", "-q", "-b", "change"]);
    std::fs::write(
        dir.join("design/mocks/A.dc.html"),
        "<x-dc><p>board A proposed</p></x-dc>",
    )
    .unwrap();
    std::fs::write(
        dir.join("design/system/components/src/bundle.js"),
        "window.Bae = { PROPOSED_MARK: 1 };",
    )
    .unwrap();
    std::fs::write(
        dir.join("design/mocks/canvas.json"),
        r#"{"v": 3, "boards": {"A.dc.html": {"w": 900, "h": 700}}}"#,
    )
    .unwrap();
    let tip = commit(&dir);
    let (change, view) = review_of(&dir, &tip);
    let data = page_data(&review_page(&change, &view, "demo").unwrap());
    let board = &data["section"]["boards"][0];
    assert_eq!(
        (&board["now"]["w"], &board["now"]["h"]),
        (&json!(400), &json!(300))
    );
    assert_eq!(
        (&board["proposed"]["w"], &board["proposed"]["h"]),
        (&json!(900), &json!(700))
    );
    let draw = |side| {
        let from = source_of(&change, side).unwrap();
        render_board(from, "A", Theme::Dark, &Props::new(), &cache(&scratch)).unwrap()
    };
    let now = draw(Side::Now);
    assert!(now.contains("hello from A") && now.contains("BUNDLE_MARK"));
    assert!(!now.contains("proposed") && !now.contains("PROPOSED_MARK"));
    let proposed = draw(Side::Proposed);
    assert!(proposed.contains("board A proposed") && proposed.contains("PROPOSED_MARK"));
    assert!(!proposed.contains("BUNDLE_MARK"));
}

#[test]
fn a_board_the_change_adds_has_no_now_and_a_board_it_deletes_has_only_a_now() {
    let scratch = Scratch::new("render-added-deleted");
    let dir = tree(&scratch);
    std::fs::write(
        dir.join("design/mocks/Old.dc.html"),
        "<x-dc><p>the old board</p></x-dc>",
    )
    .unwrap();
    std::fs::write(
        dir.join("design/mocks/canvas.json"),
        r#"{"v": 3, "boards": {"A.dc.html": {"w": 400, "h": 300}, "Old.dc.html": {"w": 50, "h": 60}, "New.dc.html": {"w": 70, "h": 80}}}"#,
    )
    .unwrap();
    let main = commit(&dir);
    set_origin_main(&dir, &main);
    git(&dir, &["checkout", "-q", "-b", "change"]);
    git(&dir, &["rm", "-q", "design/mocks/Old.dc.html"]);
    std::fs::write(
        dir.join("design/mocks/New.dc.html"),
        "<x-dc><p>new</p></x-dc>",
    )
    .unwrap();
    let tip = commit(&dir);
    let (mut change, _) = review_of(&dir, &tip);
    change.review.as_mut().unwrap().boards = vec![
        ReviewBoard {
            board: "design/mocks/A.dc.html".into(),
            caption: "the A".into(),
            theme: Theme::Dark,
            circles: vec![],
        },
        ReviewBoard {
            board: "design/mocks/New.dc.html".into(),
            caption: "the new one".into(),
            theme: Theme::Light,
            circles: vec![],
        },
    ];
    change.deleted.insert("design/mocks/Old.dc.html".into());
    let view = design_view(&crate::lists::fixtures::read(&db()), &change, None).unwrap();
    let data = page_data(&review_page(&change, &view, "demo").unwrap());
    let id = change.id.0;
    let boards = &data["section"]["boards"];
    assert_eq!(
        boards[0]["now"]["src"],
        json!(format!("/design/demo/{id}/board/0/now"))
    );
    assert_eq!(boards[1]["now"], json!({ "absent": true }));
    assert_eq!(
        boards[1]["proposed"],
        json!({ "w": 70, "h": 80, "src": format!("/design/demo/{id}/board/1/proposed") })
    );
    // The deleted board has a now, drawn from origin/main, and no proposed.
    assert_eq!(
        data["deleted"],
        json!([{
            "path": "design/mocks/Old.dc.html",
            "now": { "w": 50, "h": 60, "src": format!("/design/demo/{id}/deleted/0/now") }
        }])
    );
    assert_eq!(
        deleted_board(&change, 0).map(String::as_str),
        Some("design/mocks/Old.dc.html")
    );
    let old = render_board(
        source_of(&change, Side::Now).unwrap(),
        "design/mocks/Old.dc.html",
        Theme::Dark,
        &Props::new(),
        &cache(&scratch),
    )
    .unwrap();
    assert!(old.contains("the old board"));
}

#[test]
fn a_review_page_with_no_origin_main_says_so_at_each_now() {
    let scratch = Scratch::new("render-no-origin");
    let dir = tree(&scratch);
    let tip = commit(&dir);
    let (change, view) = review_of(&dir, &tip);
    let data = page_data(&review_page(&change, &view, "demo").unwrap());
    let board = &data["section"]["boards"][0];
    assert!(
        board["now"]["error"]
            .as_str()
            .unwrap()
            .contains("find the commit origin/main")
    );
    assert_eq!(board["proposed"]["w"], json!(400));
}

#[test]
fn a_review_page_of_a_commit_with_no_canvas_file_says_so_at_each_board() {
    let scratch = Scratch::new("render-review-nocanvas");
    let dir = tree(&scratch);
    std::fs::remove_file(dir.join("design/mocks/canvas.json")).unwrap();
    let tip = commit(&dir);
    let (change, view) = review_of(&dir, &tip);
    let data = page_data(&review_page(&change, &view, "demo").unwrap());
    assert!(
        data["section"]["boards"][0]["proposed"]["error"]
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
        dir.join("design/system/components/src/bundle.js"),
        "window.Bae = { OTHER_MARK: 1 };",
    )
    .unwrap();
    let second = commit(&dir);
    std::fs::write(dir.join("design/mocks/A.dc.html"), "<x-dc>edited</x-dc>").unwrap();

    let at = |rev: &str| {
        render_board(
            Source::Commit { repo: &dir, rev },
            "A",
            Theme::Dark,
            &Props::new(),
            &cache(&scratch),
        )
        .unwrap()
    };
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
        &Props::new(),
        &cache(&scratch),
    ));
    assert!(missing.contains("find the commit nope"), "{missing}");
    let absent = message(render_board(
        Source::Commit {
            repo: &dir,
            rev: &first,
        },
        "Nope",
        Theme::Dark,
        &Props::new(),
        &cache(&scratch),
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
        let data = page_data(&review_page(&change, &view, "demo").unwrap());
        data["section"]["boards"][0]["proposed"].clone()
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
    let data = page_data(&review_page(&change, &view, "demo").unwrap());
    let error = data["section"]["boards"][0]["proposed"]["error"]
        .as_str()
        .unwrap();
    assert!(error.contains("amend it"), "{error}");
}

/// Runs <expression> after the review page's script (`circles.js`) in node
/// and returns what it prints as JSON.
fn run_circles(expression: &str) -> Value {
    let program = format!(
        "{}\nconsole.log(JSON.stringify({expression}));",
        CIRCLES_SCRIPT
    );
    let out = std::process::Command::new("node")
        .arg("-e")
        .arg(program)
        .output()
        .expect("these tests run the page's script in node, which must be installed");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Runs `circleMarks` over a board's circles and what each side found, and
/// returns its marks.
fn circle_marks(circles: &Value, now: &Value, proposed: &Value) -> Value {
    run_circles(&format!("circleMarks({circles}, {now}, {proposed})"))
}

/// What `boardStatus` says under a board's caption.
fn board_status(is_new: bool, labels: &[&str]) -> String {
    let said = run_circles(&format!("boardStatus({is_new}, {})", json!(labels)));
    said.as_str().expect("a status is text").to_string()
}

#[test]
fn a_new_boards_status_keeps_its_words_apart_from_the_circles() {
    assert_eq!(
        board_status(true, &["The greeting", "The button"]),
        "New board. Circled: The greeting; The button"
    );
    assert_eq!(board_status(true, &[]), "New board.");
    assert_eq!(
        board_status(false, &["The greeting"]),
        "Circled: The greeting"
    );
    assert_eq!(board_status(false, &[]), "");
}

fn found(boxes: &[(i64, i64)]) -> Value {
    json!({ "boxes": boxes.iter().map(|(x, y)| json!({ "x": x, "y": y, "w": 100, "h": 20 })).collect::<Vec<_>>() })
}

#[test]
fn each_circle_marks_what_its_selector_finds_on_each_side_with_its_label() {
    let circles = json!([
        { "selector": "#disc-1", "name": "#disc-1", "label": "The Disc 1 heading goes" },
        { "selector": ".row:first-child", "name": ".row:first-child" }
    ]);
    // The heading is only on now; the first row is on both sides, moved up.
    let now = json!([found(&[(10, 10)]), found(&[(10, 40)])]);
    let proposed = json!([found(&[]), found(&[(10, 10)])]);
    assert_eq!(
        circle_marks(&circles, &now, &proposed),
        json!({
            "now": [
                { "box": { "x": 10, "y": 10, "w": 100, "h": 20 }, "title": "The Disc 1 heading goes" },
                { "box": { "x": 10, "y": 40, "w": 100, "h": 20 }, "title": ".row:first-child" }
            ],
            "proposed": [
                { "box": { "x": 10, "y": 10, "w": 100, "h": 20 }, "title": ".row:first-child" }
            ],
            "problems": []
        })
    );
}

#[test]
fn a_box_circle_marks_its_box_on_each_side_the_page_draws_and_finds_no_selector() {
    let circles = json!([
        { "box": { "x": 10, "y": 20, "width": 300, "height": 40 }, "name": "box 10,20 300x40", "label": "The sheet" },
        { "box": { "x": 1, "y": 2, "width": 3, "height": 4 }, "name": "box 1,2 3x4" },
        { "selector": ".row", "name": ".row" }
    ]);
    // The board page answers an empty list for a box, as it is no selector.
    let now = json!([found(&[]), found(&[]), found(&[(5, 6)])]);
    let marks = circle_marks(&circles, &now, &Value::Null);
    assert_eq!(
        marks,
        json!({
            "now": [
                { "box": { "x": 10, "y": 20, "w": 300, "h": 40 }, "title": "The sheet" },
                { "box": { "x": 1, "y": 2, "w": 3, "h": 4 }, "title": "box 1,2 3x4" },
                { "box": { "x": 5, "y": 6, "w": 100, "h": 20 }, "title": ".row" }
            ],
            "proposed": [],
            "problems": []
        })
    );
}

#[test]
fn a_side_that_has_answered_gets_its_marks_before_the_other_answers() {
    let circles = json!([
        { "selector": ".row", "name": ".row" },
        { "box": { "x": 1, "y": 2, "width": 3, "height": 4 }, "name": "box 1,2 3x4" }
    ]);
    // Only the proposed side has answered: it has its marks and the other none.
    let proposed = json!([found(&[(1, 2)]), found(&[])]);
    let marks = circle_marks(&circles, &Value::Null, &proposed);
    assert_eq!(marks["now"], json!([]));
    assert_eq!(marks["proposed"].as_array().unwrap().len(), 2);
}

#[test]
fn a_circle_that_finds_nothing_or_is_no_selector_is_a_problem_and_a_side_with_no_drawing_is_skipped()
 {
    let circles = json!([
        { "selector": "#gone", "name": "#gone" },
        { "selector": "[[", "name": "[[" },
        { "selector": "#here", "name": "#here" }
    ]);
    let proposed = json!([
        found(&[]),
        { "error": "'[[' is not a valid selector" },
        found(&[(1, 2)])
    ]);
    let marks = circle_marks(&circles, &Value::Null, &proposed);
    assert_eq!(marks["now"], json!([]));
    assert_eq!(marks["proposed"].as_array().unwrap().len(), 1);
    assert_eq!(
        marks["problems"],
        json!([
            "The circle #gone finds no element on the board.",
            "The circle [[ is no CSS selector: '[[' is not a valid selector"
        ])
    );
}

#[test]
fn the_review_page_gives_each_board_its_circles_and_each_board_page_answers_what_selectors_find() {
    let scratch = Scratch::new("render-circles");
    let dir = tree(&scratch);
    let tip = commit(&dir);
    set_origin_main(&dir, &tip);
    let (mut change, view) = review_of(&dir, &tip);
    let circle = Circle {
        around: Around::Selector(".a-only".into()),
        label: Some("The greeting".into()),
    };
    change.review.as_mut().unwrap().boards[0].circles = vec![circle];
    let page = review_page(&change, &view, "demo").unwrap();
    assert!(page.contains("function circleMarks") && !page.contains("@@CIRCLES@@"));
    let data = page_data(&page);
    assert_eq!(
        data["section"]["boards"][0]["circles"],
        json!([{ "selector": ".a-only", "label": "The greeting", "name": ".a-only" }])
    );
    assert_eq!(data["section"]["boards"][1]["circles"], json!([]));
    let board = render_board(
        source_of(&change, Side::Proposed).unwrap(),
        "A",
        Theme::Dark,
        &Props::new(),
        &cache(&scratch),
    )
    .unwrap();
    assert!(board.contains("function foundBy") && board.contains("designBoxes"));
}

/// Runs `variantCrop` of the review page's script (`variants.js`) over what a
/// variant's board page found for the selector on a board of <w> by <h>.
fn variant_crop(found: &Value, w: i64, h: i64) -> Value {
    let program = format!(
        "{}\nconsole.log(JSON.stringify(variantCrop(\".fields\", {found}, {w}, {h})));",
        VARIANTS_SCRIPT
    );
    let out = std::process::Command::new("node")
        .arg("-e")
        .arg(program)
        .output()
        .expect("these tests run the page's script in node, which must be installed");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn a_variant_crops_to_the_first_element_its_selector_finds_with_a_margin_inside_the_board() {
    // The first of two elements, 16 pixels clear of it on each side.
    assert_eq!(
        variant_crop(
            &json!({ "boxes": [{ "x": 40, "y": 50, "w": 100, "h": 20 }, { "x": 0, "y": 0, "w": 5, "h": 5 }] }),
            400,
            300
        ),
        json!({ "crop": { "x": 24, "y": 34, "w": 132, "h": 52 } })
    );
    // Near the board's corner, the margin stops at the board's edges.
    assert_eq!(
        variant_crop(
            &json!({ "boxes": [{ "x": 4, "y": 2, "w": 390, "h": 290 }] }),
            400,
            300
        ),
        json!({ "crop": { "x": 0, "y": 0, "w": 400, "h": 300 } })
    );
    assert_eq!(
        variant_crop(&json!({ "boxes": [] }), 400, 300),
        json!({ "problem": "The selector .fields finds no element on this variant." })
    );
    assert_eq!(
        variant_crop(
            &json!({ "error": "'[[' is not a valid selector" }),
            400,
            300
        ),
        json!({ "problem": "The selector .fields is no CSS selector: '[[' is not a valid selector" })
    );
    assert_eq!(
        variant_crop(
            &json!({ "boxes": [{ "x": 900, "y": 10, "w": 10, "h": 10 }] }),
            400,
            300
        ),
        json!({ "problem": "The selector .fields finds an element outside the board." })
    );
}

#[test]
fn the_review_page_gives_each_variant_its_label_and_frame_and_each_variant_draws_with_its_props() {
    let scratch = Scratch::new("render-variants");
    let dir = tree(&scratch);
    std::fs::write(
        dir.join("design/mocks/A.dc.html"),
        r#"<x-dc><p class="fields">{{word}}</p></x-dc>
<script type="text/x-dc">class Component extends DCLogic { renderVals() { return { word: this.props.storage ?? "none" }; } }</script>"#,
    )
    .unwrap();
    let tip = commit(&dir);
    set_origin_main(&dir, &tip);
    let (mut change, view) = review_of(&dir, &tip);
    let props = |value: Value| value.as_object().unwrap().clone();
    change.variants = Some(Variants {
        board: "design/mocks/A.dc.html".into(),
        selector: ".fields".into(),
        theme: Theme::Light,
        tip: tip.clone(),
        props: vec![
            props(json!({ "storage": "dropbox" })),
            props(json!({ "storage": "</script>", "rows": 2 })),
        ],
    });
    let page = review_page(&change, &view, "demo").unwrap();
    assert!(page.contains("function variantCrop") && !page.contains("@@VARIANTS@@"));
    let id = change.id.0;
    assert_eq!(
        page_data(&page)["variants"],
        json!({
            "board": "design/mocks/A.dc.html",
            "selector": ".fields",
            "theme": "light",
            "tip": tip,
            "drawings": [
                { "label": "storage: dropbox", "w": 400, "h": 300, "src": format!("/design/demo/{id}/variant/0") },
                { "label": "rows: 2, storage: </script>", "w": 400, "h": 300, "src": format!("/design/demo/{id}/variant/1") }
            ]
        })
    );
    // Each variant's page draws the board with its props, and no prop's text ends a script.
    let variants = change.variants.as_ref().unwrap();
    let draw = |k: usize| {
        render_board(
            variants_source(&change, variants),
            &variants.board,
            variants.theme,
            &variants.props[k],
            &cache(&scratch),
        )
        .unwrap()
    };
    let first = draw(0);
    assert!(
        first.contains(r#"var PROPS = {"storage":"dropbox"};"#)
            && first.contains("this.props.storage"),
        "{first}"
    );
    assert!(draw(1).contains(r#"var PROPS = {"rows":2,"storage":"\u003c/script>"};"#));
    assert!(
        render_board(
            Source::Dir(&dir),
            "A",
            Theme::Dark,
            &Props::new(),
            &cache(&scratch)
        )
        .unwrap()
        .contains("var PROPS = {};")
    );

    // A board with no size at the variants' commit is said at each drawing, under its label.
    change.variants.as_mut().unwrap().board = "design/mocks/B.dc.html".into();
    let drawings =
        &page_data(&review_page(&change, &view, "demo").unwrap())["variants"]["drawings"];
    assert_eq!(drawings[1]["label"], json!("rows: 2, storage: </script>"));
    let error = drawings[0]["error"].as_str().unwrap();
    assert!(
        error.contains("no width and height for B.dc.html"),
        "{error}"
    );
    // A change with no variants has none on its page.
    change.variants = None;
    assert_eq!(
        page_data(&review_page(&change, &view, "demo").unwrap())["variants"],
        Value::Null
    );
}
