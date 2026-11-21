use super::*;
use crate::lists::design::{Around, Circle, ReviewSection};
use crate::lists::fixtures::{db, design_drawn, design_tipped, read};
use crate::lists::view::design_view;
use crate::testing::{Scratch, design_system, git, repo, run_script};
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
            heading: "A heading".into(),
            boards: vec![
                ReviewEntry {
                    shown: Shown::Board {
                        path: "design/mocks/A.dc.html".into(),
                        theme: Theme::Dark,
                    },
                    caption: "the A </script> caption".into(),
                    show_now: true,
                    circles: vec![],
                },
                ReviewEntry {
                    shown: Shown::Board {
                        path: "design/mocks/B.dc.html".into(),
                        theme: Theme::Light,
                    },
                    caption: "no size".into(),
                    show_now: true,
                    circles: vec![],
                },
                ReviewEntry {
                    shown: Shown::Board {
                        path: "design/mocks/Z.dc.html".into(),
                        theme: Theme::Light,
                    },
                    caption: "no file".into(),
                    show_now: true,
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
fn a_review_page_has_each_board_now_and_proposed_sized_from_the_canvas() {
    let scratch = Scratch::new("render-review");
    let dir = tree(&scratch);
    let tip = commit(&dir);
    set_origin_main(&dir, &tip);
    let (change, view) = review_of(&dir, &tip);
    let page = review_page(&change, &view, "demo", &Value::Null).unwrap();
    assert!(
        !page.contains("A </script> caption"),
        "text can't end the data script"
    );
    let data = page_data(&page);
    assert_eq!(
        data["section"]["boards"][0]["caption"],
        json!("the A </script> caption")
    );
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
    let data = page_data(&review_page(&change, &view, "demo", &Value::Null).unwrap());
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
        ReviewEntry {
            shown: Shown::Board {
                path: "design/mocks/A.dc.html".into(),
                theme: Theme::Dark,
            },
            caption: "the A".into(),
            show_now: true,
            circles: vec![],
        },
        ReviewEntry {
            shown: Shown::Board {
                path: "design/mocks/New.dc.html".into(),
                theme: Theme::Light,
            },
            caption: "the new one".into(),
            show_now: true,
            circles: vec![],
        },
    ];
    change.deleted.insert("design/mocks/Old.dc.html".into());
    let view = design_view(&crate::lists::fixtures::read(&db()), &change, None).unwrap();
    let data = page_data(&review_page(&change, &view, "demo", &Value::Null).unwrap());
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

/// A component's preview on the page shows its now from origin/main beside
/// its proposal from the change's last commit, as a board does; a component
/// the change adds has no now, and one with no preview at the change's last
/// commit is the change's problem. Each frame loads the same address a board's
/// does, and the page marks it as a preview that reports its own size.
#[test]
fn a_review_page_shows_a_components_preview_now_and_proposed() {
    let scratch = Scratch::new("render-component");
    let dir = tree(&scratch);
    let preview = |name: &str, mark: &str| {
        let path = dir.join(format!("design/system/components/{name}/preview.html"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("<!-- @dsCard height=100 -->\n<html><head></head><body>{mark}</body></html>"),
        )
        .unwrap();
    };
    preview("MacRow", "row as it stands");
    git(&dir, &["add", "-f", "design"]);
    let main = commit(&dir);
    set_origin_main(&dir, &main);
    git(&dir, &["checkout", "-q", "-b", "change"]);
    preview("MacRow", "row as proposed");
    preview("MacNew", "the new row");
    git(&dir, &["add", "-f", "design"]);
    let tip = commit(&dir);
    let (mut change, _) = review_of(&dir, &tip);
    let component = |name: &str| ReviewEntry {
        shown: Shown::Component(name.into()),
        caption: format!("{name}'s preview"),
        show_now: true,
        circles: vec![Circle {
            around: Around::Selector(".row".into()),
            label: None,
        }],
    };
    change.review.as_mut().unwrap().boards = vec![
        component("MacRow"),
        component("MacNew"),
        component("MacGone"),
    ];
    let view = design_view(&read(&db()), &change, None).unwrap();
    let data = page_data(&review_page(&change, &view, "demo", &Value::Null).unwrap());
    let id = change.id.0;
    let entries = &data["section"]["boards"];
    assert_eq!(entries[0]["component"], json!("MacRow"));
    assert_eq!(entries[0]["path"], Value::Null);
    assert_eq!(entries[0]["circles"][0]["selector"], json!(".row"));
    for side in ["proposed", "now"] {
        let drawn = &entries[0][side];
        assert_eq!(drawn["preview"], json!(true), "{side}");
        assert_eq!(
            drawn["src"],
            json!(format!("/design/demo/{id}/board/0/{side}"))
        );
        assert_eq!(drawn["w"], json!(PREVIEW_START.0));
    }
    assert_eq!(entries[1]["now"], json!({ "absent": true }));
    assert_eq!(entries[1]["proposed"]["preview"], json!(true));
    assert_eq!(entries[2]["now"], json!({ "absent": true }));
    assert_eq!(
        entries[2]["proposed"]["error"],
        json!("MacGone has no preview at the change's last commit")
    );
    let sides = |side| {
        crate::daemon::designs::review_preview(
            source_of(&change, side).unwrap(),
            "MacRow",
            &cache(&scratch),
        )
        .unwrap()
    };
    let now = sides(Side::Now);
    assert!(now.contains("row as it stands") && now.contains("BUNDLE_MARK"));
    let proposed = sides(Side::Proposed);
    assert!(proposed.contains("row as proposed") && !proposed.contains("as it stands"));
}

/// A board on the page shows its proposal alone unless the recorder asked
/// for its now, and the page lists each other board the change changes or
/// deletes by name, with the drawing the reviewer opens it to: a changed
/// board's proposal, a deleted board's now (decision #369).
#[test]
fn a_review_page_shows_the_proposal_and_lists_the_other_boards_of_the_change() {
    let scratch = Scratch::new("render-proposal-only");
    let dir = tree(&scratch);
    std::fs::write(
        dir.join("design/mocks/Old.dc.html"),
        "<x-dc><p>the old board</p></x-dc>",
    )
    .unwrap();
    std::fs::write(
        dir.join("design/mocks/canvas.json"),
        r#"{"v": 3, "boards": {"A.dc.html": {"w": 400, "h": 300}, "B.dc.html": {"w": 20, "h": 30}, "C.dc.html": {"w": 40, "h": 50}, "Old.dc.html": {"w": 50, "h": 60}}}"#,
    )
    .unwrap();
    let main = commit(&dir);
    set_origin_main(&dir, &main);
    git(&dir, &["checkout", "-q", "-b", "change"]);
    git(&dir, &["rm", "-q", "design/mocks/Old.dc.html"]);
    std::fs::write(dir.join("design/mocks/C.dc.html"), "<x-dc><p>C</p></x-dc>").unwrap();
    let tip = commit(&dir);
    let db = db();
    let id = design_drawn(
        &db,
        &dir.display().to_string(),
        &[
            ("design/mocks/A.dc.html", "https://canvas.example/#A"),
            ("design/mocks/C.dc.html", "https://canvas.example/#C"),
            ("design/mocks/B.dc.html", "https://canvas.example/#B"),
        ],
        Some(ReviewSection {
            heading: "A heading".into(),
            boards: vec![ReviewEntry {
                shown: Shown::Board {
                    path: "design/mocks/A.dc.html".into(),
                    theme: Theme::Dark,
                },
                caption: "the A".into(),
                show_now: false,
                circles: vec![],
            }],
        }),
    );
    design_tipped(&db, id, &tip);
    let mut change = read(&db).design(id).unwrap().clone();
    change.deleted.insert("design/mocks/Old.dc.html".into());
    let view = design_view(&read(&db), &change, None).unwrap();
    let data = page_data(&review_page(&change, &view, "demo", &Value::Null).unwrap());
    let id = change.id.0;
    let board = &data["section"]["boards"][0];
    assert_eq!(board["now"], Value::Null, "{board}");
    assert_eq!(
        board["proposed"]["src"],
        json!(format!("/design/demo/{id}/board/0/proposed"))
    );
    // B and C are changed and not on the page; Old is deleted.
    assert_eq!(
        data["others"],
        json!([
            {
                "path": "design/mocks/B.dc.html",
                "proposed": { "w": 20, "h": 30, "src": format!("/design/demo/{id}/changed/0/proposed") }
            },
            {
                "path": "design/mocks/C.dc.html",
                "proposed": { "w": 40, "h": 50, "src": format!("/design/demo/{id}/changed/1/proposed") }
            },
        ])
    );
    assert_eq!(
        data["deleted"][0]["now"]["src"],
        json!(format!("/design/demo/{id}/deleted/0/now"))
    );
    assert_eq!(
        other_board(&change, 1).map(String::as_str),
        Some("design/mocks/C.dc.html")
    );
    assert!(other_board(&change, 2).is_none());
}

#[test]
fn a_review_page_with_no_origin_main_says_so_at_each_now() {
    let scratch = Scratch::new("render-no-origin");
    let dir = tree(&scratch);
    let tip = commit(&dir);
    let (change, view) = review_of(&dir, &tip);
    let data = page_data(&review_page(&change, &view, "demo", &Value::Null).unwrap());
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
    let data = page_data(&review_page(&change, &view, "demo", &Value::Null).unwrap());
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
        let data = page_data(&review_page(&change, &view, "demo", &Value::Null).unwrap());
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
    let data = page_data(&review_page(&change, &view, "demo", &Value::Null).unwrap());
    let error = data["section"]["boards"][0]["proposed"]["error"]
        .as_str()
        .unwrap();
    assert!(error.contains("amend it"), "{error}");
}

/// Runs `circleMarks` over a board's circles and what each side found, and
/// returns its marks.
fn circle_marks(circles: &Value, now: &Value, proposed: &Value) -> Value {
    run_script(
        CIRCLES_SCRIPT,
        &format!("circleMarks({circles}, {now}, {proposed})"),
    )
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
                { "box": { "x": 10, "y": 10, "w": 100, "h": 20 }, "title": "The Disc 1 heading goes", "label": "The Disc 1 heading goes" },
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
                { "box": { "x": 10, "y": 20, "w": 300, "h": 40 }, "title": "The sheet", "label": "The sheet" },
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
    let page = review_page(&change, &view, "demo", &Value::Null).unwrap();
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
        &cache(&scratch),
    )
    .unwrap();
    assert!(board.contains("function foundBy") && board.contains("designBoxes"));
}
