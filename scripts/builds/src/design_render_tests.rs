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
    std::fs::remove_file(dir.join("design/system/components/bundle.js")).unwrap();
    assert!(
        message(render_board(
            Source::Dir(&dir),
            "A",
            Theme::Dark,
            &cache(&scratch)
        ))
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
fn a_review_page_has_its_sections_and_each_board_now_and_proposed_sized_from_the_canvas() {
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
    assert_eq!(data["sections"][0]["heading"], json!("A </script> heading"));
    assert_eq!(data["sections"][1]["boards"], json!([]));
    let boards = &data["sections"][0]["boards"];
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
    let board = &data["sections"][0]["boards"][0];
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
    change.review[0].boards = vec![
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
    let boards = &data["sections"][0]["boards"];
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
    let board = &data["sections"][0]["boards"][0];
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
        data["sections"][0]["boards"][0]["proposed"]["error"]
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
        data["sections"][0]["boards"][0]["proposed"].clone()
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
    let error = data["sections"][0]["boards"][0]["proposed"]["error"]
        .as_str()
        .unwrap();
    assert!(error.contains("amend it"), "{error}");
}

#[test]
fn the_tokens_are_read_from_a_directory_or_a_commit_and_an_error_names_their_file() {
    let scratch = Scratch::new("render-tokens");
    let dir = tree(&scratch);
    let tokens = dir.join("design/system/tokens.json");
    assert!(
        Source::Dir(&dir)
            .tokens_css()
            .unwrap()
            .contains("--tokens: 1;")
    );
    let first = commit(&dir);
    std::fs::write(&tokens, crate::testing::tokens_json("2")).unwrap();
    let at = |rev: &str| Source::Commit { repo: &dir, rev }.tokens_css();
    assert!(at(&first).unwrap().contains("--tokens: 1;"));
    assert!(
        Source::Dir(&dir)
            .tokens_css()
            .unwrap()
            .contains("--tokens: 2;")
    );
    std::fs::write(&tokens, "not json").unwrap();
    assert!(
        message(Source::Dir(&dir).tokens_css())
            .starts_with(&format!("{}: reading the tokens", tokens.display()))
    );
    let broken = commit(&dir);
    assert!(message(at(&broken)).starts_with(&format!(
        "{broken}:design/system/tokens.json: reading the tokens"
    )));
    std::fs::remove_file(&tokens).unwrap();
    assert_eq!(
        message(Source::Dir(&dir).tokens_css()),
        format!(
            "the design system's tokens {} is not there",
            tokens.display()
        )
    );
}

/// The design systems kept in <cache>, one folder each.
fn kept(cache: &Path) -> usize {
    std::fs::read_dir(cache)
        .map(|entries| {
            entries
                .filter(|entry| {
                    !entry
                        .as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with('.')
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_commits_design_system_is_generated_from_its_own_sources_once() {
    let scratch = Scratch::new("render-generated");
    let dir = tree(&scratch);
    let first = commit(&dir);
    let at = |rev: &str| {
        render_board(
            Source::Commit { repo: &dir, rev },
            "A",
            Theme::Dark,
            &cache(&scratch),
        )
        .unwrap()
    };
    // The commit holds no generated file; the page carries what its sources generate.
    assert!(!exists_in(&dir, &first, "design/system/tokens.css").unwrap());
    assert!(!exists_in(&dir, &first, "design/system/components/bundle.js").unwrap());
    let page = at(&first);
    assert!(page.contains("--tokens: 1;") && page.contains(".bundle { color: red; }"));
    assert!(page.contains("BUNDLE_MARK"));
    assert_eq!(kept(&cache(&scratch)), 1);
    // Drawing it again, or a commit that changes only a board, generates nothing new.
    at(&first);
    std::fs::write(
        dir.join("design/mocks/B.dc.html"),
        "<x-dc><p>B again</p></x-dc>",
    )
    .unwrap();
    at(&commit(&dir));
    assert_eq!(kept(&cache(&scratch)), 1);
    // A commit that changes the tokens gets its own.
    std::fs::write(
        dir.join("design/system/tokens.json"),
        crate::testing::tokens_json("2"),
    )
    .unwrap();
    let second = commit(&dir);
    assert!(at(&second).contains("--tokens: 2;"));
    assert!(at(&first).contains("--tokens: 1;"));
    assert_eq!(kept(&cache(&scratch)), 2);
}

#[test]
fn a_commit_whose_design_system_cannot_be_generated_says_why() {
    let scratch = Scratch::new("render-ungenerated");
    let dir = tree(&scratch);
    std::fs::write(dir.join("design/system/tokens.json"), "not json").unwrap();
    let broken = commit(&dir);
    let error = message(render_board(
        Source::Commit {
            repo: &dir,
            rev: &broken,
        },
        "A",
        Theme::Dark,
        &cache(&scratch),
    ));
    assert!(
        error.contains(&format!(
            "{broken}:design/system/tokens.json: reading the tokens"
        )),
        "{error}"
    );
    let draw = |rev: &str| {
        message(render_board(
            Source::Commit { repo: &dir, rev },
            "A",
            Theme::Dark,
            &cache(&scratch),
        ))
    };
    std::fs::write(
        dir.join("design/system/tokens.json"),
        crate::testing::tokens_json("1"),
    )
    .unwrap();
    std::fs::write(
        dir.join("design/system/build-bundle.py"),
        "raise SystemExit('no bundles')",
    )
    .unwrap();
    let error = draw(&commit(&dir));
    assert!(
        error.contains("design/system/build-bundle.py failed") && error.contains("no bundles"),
        "{error}"
    );
    // The renderer refuses a source that links outside the repo and does not
    // pass it to build-bundle.py.
    std::os::unix::fs::symlink("/etc/hosts", dir.join("design/system/outside")).unwrap();
    let error = draw(&commit(&dir));
    assert!(
        error.contains("cannot be generated") && error.contains("points outside the repo"),
        "{error}"
    );
    assert_eq!(kept(&cache(&scratch)), 0);
}

#[test]
fn a_design_system_source_linked_from_elsewhere_in_the_repo_is_exported_with_it() {
    let scratch = Scratch::new("render-linked");
    let dir = tree(&scratch);
    // The bundle's source is a link to a file at the repo's top, as bae's sheep logo is.
    std::fs::write(dir.join("linked.js"), "window.Bae = { LINKED_MARK: 1 };").unwrap();
    let source = dir.join("design/system/components/src/bundle.js");
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink("../../../../linked.js", &source).unwrap();
    git(&dir, &["add", "linked.js"]);
    let linked = commit(&dir);
    let page = render_board(
        Source::Commit {
            repo: &dir,
            rev: &linked,
        },
        "A",
        Theme::Dark,
        &cache(&scratch),
    )
    .unwrap();
    assert!(page.contains("LINKED_MARK"));
    // A change to the linked file is another design system.
    std::fs::write(dir.join("linked.js"), "window.Bae = { RELINKED_MARK: 1 };").unwrap();
    git(&dir, &["add", "linked.js"]);
    git(&dir, &["commit", "-q", "-m", "relinked"]);
    let page = render_board(
        Source::Commit {
            repo: &dir,
            rev: "HEAD",
        },
        "A",
        Theme::Dark,
        &cache(&scratch),
    )
    .unwrap();
    assert!(page.contains("RELINKED_MARK"));
}
