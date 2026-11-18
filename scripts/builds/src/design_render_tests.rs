use super::*;
use crate::lists::design::ReviewSection;
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
    let page = render_board(Source::Dir(&dir), "A", Theme::Light, &cache(&scratch)).unwrap();
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
            &cache(&scratch)
        ))
        .contains("Nope.dc.html is not there")
    );
    assert!(
        message(render_board(
            Source::Dir(&dir),
            "../A",
            Theme::Dark,
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
        render_board(from, "A", Theme::Dark, &cache(&scratch)).unwrap()
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
        },
        ReviewBoard {
            board: "design/mocks/New.dc.html".into(),
            caption: "the new one".into(),
            theme: Theme::Light,
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

/// Runs `changesBetween` of the review page's script (`changes.js`) over two
/// element lists in node, and returns its marks.
fn changes_between(now: &Value, proposed: &Value) -> Value {
    let program = format!(
        "{}\nconsole.log(JSON.stringify(changesBetween({now}, {proposed})));",
        CHANGES_SCRIPT
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

fn element(text: &str, role: &str, x: i64, y: i64) -> Value {
    json!({ "text": text, "role": role, "x": x, "y": y, "w": 100, "h": 20,
        "color": "rgb(0, 0, 0)", "background": "" })
}

#[test]
fn a_board_that_did_not_change_has_no_marks() {
    let list = json!([
        element("Devices", "text", 10, 10),
        element("Remove", "button", 10, 40)
    ]);
    assert_eq!(
        changes_between(&list, &list),
        json!({ "now": [], "proposed": [] })
    );
}

#[test]
fn an_element_only_in_now_is_missing_and_one_only_in_proposed_is_new() {
    let now = json!([
        element("Devices", "text", 10, 10),
        element("Old", "button", 10, 40)
    ]);
    let proposed = json!([
        element("Devices", "text", 10, 10),
        element("Added", "button", 10, 80)
    ]);
    let marks = changes_between(&now, &proposed);
    assert_eq!(marks["now"].as_array().unwrap().len(), 1);
    assert_eq!(marks["now"][0]["kind"], json!("missing"));
    assert_eq!(
        marks["now"][0]["title"],
        json!("Was: Old (not in the proposal)")
    );
    assert_eq!(marks["proposed"].as_array().unwrap().len(), 1);
    assert_eq!(marks["proposed"][0]["kind"], json!("new"));
    assert_eq!(
        marks["proposed"][0]["box"],
        json!({ "x": 10, "y": 80, "w": 100, "h": 20 })
    );
}

#[test]
fn an_element_with_other_words_in_the_same_place_is_one_changed_element_that_says_what_it_was() {
    let now = json!([element("Removing the device", "text", 10, 10)]);
    let proposed = json!([element("Remove the device", "text", 10, 10)]);
    let marks = changes_between(&now, &proposed);
    assert_eq!(marks["proposed"].as_array().unwrap().len(), 1);
    assert_eq!(marks["proposed"][0]["kind"], json!("restyled"));
    assert_eq!(
        marks["proposed"][0]["title"],
        json!("Was: Removing the device")
    );
    assert_eq!(
        marks["now"][0]["title"],
        json!("Becomes: Remove the device")
    );
}

#[test]
fn a_box_that_moves_more_than_2_px_is_moved_and_a_colour_change_is_restyled() {
    let now = json!([element("A", "text", 10, 10), element("B", "text", 10, 50)]);
    let mut moved = element("A", "text", 10, 12);
    let near = changes_between(&now, &json!([moved.clone(), element("B", "text", 10, 50)]));
    assert_eq!(near, json!({ "now": [], "proposed": [] }));
    moved["y"] = json!(30);
    let mut recoloured = element("B", "text", 10, 50);
    recoloured["color"] = json!("rgb(255, 0, 0)");
    let marks = changes_between(&now, &json!([moved, recoloured]));
    let of_kind = |kind: &str| {
        marks["proposed"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["kind"] == json!(kind))
            .unwrap()
            .clone()
    };
    assert_eq!(marks["proposed"].as_array().unwrap().len(), 2);
    assert_eq!(of_kind("moved")["title"], json!("Moved 20 px down"));
    assert_eq!(
        of_kind("restyled")["title"],
        json!("Text colour was rgb(0, 0, 0)")
    );
}

#[test]
fn elements_with_the_same_words_pair_with_the_nearest_one() {
    let now = json!([
        element("Row", "text", 10, 10),
        element("Row", "text", 10, 100)
    ]);
    let proposed = json!([
        element("Row", "text", 10, 100),
        element("Row", "text", 10, 10)
    ]);
    assert_eq!(
        changes_between(&now, &proposed),
        json!({ "now": [], "proposed": [] })
    );
}

#[test]
fn the_review_page_carries_the_script_that_circles_changes_and_each_board_page_lists_its_elements()
{
    let scratch = Scratch::new("render-circles");
    let dir = tree(&scratch);
    let tip = commit(&dir);
    set_origin_main(&dir, &tip);
    let (change, view) = review_of(&dir, &tip);
    let page = review_page(&change, &view, "demo").unwrap();
    assert!(page.contains("function changesBetween") && !page.contains("@@CHANGES@@"));
    let board = render_board(
        source_of(&change, Side::Proposed).unwrap(),
        "A",
        Theme::Dark,
        &cache(&scratch),
    )
    .unwrap();
    assert!(board.contains("function drawnElements") && board.contains("designElements"));
}
