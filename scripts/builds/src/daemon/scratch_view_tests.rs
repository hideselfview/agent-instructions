use super::*;
use crate::testing::{Scratch, design_system, git, repo};

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().text()
}

fn globs() -> Globs {
    Globs {
        places: vec!["design/mocks/*.dc.html".into()],
        sources: vec!["design/system/tokens.json".into()],
        ..Globs::default()
    }
}

/// Repo `bae`'s main checkout in the scratch home, whose origin/main holds
/// board `A` at 300 by 200 on the canvas's one page and the design system,
/// and its worktree `side-1`, whose files hold an edit of `A` at 500 by 100
/// that no commit holds, a board `N` that origin/main has no file of, and
/// the scratch board `scratch.rows`. Returns the main checkout and the
/// worktree.
pub(in crate::daemon) fn checkouts(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let main = scratch.repo_path("bae");
    let mocks = main.join("design/mocks");
    std::fs::create_dir_all(&mocks).unwrap();
    design_system(&main, "1", "", "window.Bae = {};");
    std::fs::write(mocks.join("A.dc.html"), "<x-dc><p>board A</p></x-dc>").unwrap();
    std::fs::write(
        mocks.join("canvas.json"),
        r#"{"pages": [{"id": "one", "name": "One"}], "boards": {"A.dc.html": {"x": 0, "y": 0, "w": 300, "h": 200}}}"#,
    )
    .unwrap();
    repo(&main, "");
    git(&main, &["add", "design"]);
    git(&main, &["commit", "-q", "-m", "boards"]);
    git(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let side = worktrees_folder(&main).unwrap().join("side-1");
    git(
        &main,
        &["worktree", "add", "-q", "--detach", side.to_str().unwrap()],
    );
    let mocks = side.join("design/mocks");
    std::fs::write(
        mocks.join("A.dc.html"),
        "<x-dc><p>board A edited</p></x-dc>",
    )
    .unwrap();
    std::fs::write(mocks.join("N.dc.html"), "<x-dc><p>board N</p></x-dc>").unwrap();
    std::fs::write(
        mocks.join("scratch.rows.dc.html"),
        "<x-dc><p>rows one</p><p>rows two</p></x-dc>",
    )
    .unwrap();
    std::fs::write(
        mocks.join("canvas.json"),
        r#"{"boards": {"A.dc.html": {"w": 500, "h": 100}, "N.dc.html": {"w": 40, "h": 30}}}"#,
    )
    .unwrap();
    (main, side)
}

#[test]
fn a_scratch_view_address_names_a_project_a_worktree_and_a_board() {
    let target = Target {
        project: "bae",
        worktree: "side-4",
        board: "mac.welcome",
    };
    assert_eq!(
        route("/design/bae/scratch/side-4/mac.welcome"),
        Some(Route::Page(target))
    );
    assert_eq!(
        route("/design/bae/scratch/side-4/mac.welcome/events"),
        Some(Route::Events(target))
    );
    assert_eq!(
        route("/design/bae/scratch/side-4/mac.welcome/light/now"),
        Some(Route::Drawing(target, Theme::Light, Side::Now))
    );
    assert_eq!(
        route("/design/bae/scratch/side-4/mac.welcome/dark/worktree"),
        Some(Route::Drawing(target, Theme::Dark, Side::Worktree))
    );
    for other in [
        "/design/bae/7",
        "/design/bae/scratch/side-4",
        "/design/bae/scratch/side-4/mac.welcome/other",
        "/design/bae/scratch/side-4/mac.welcome/grey/now",
        "/design/bae/scratch/side-4/mac.welcome/dark/proposed",
        "/design/bae/scratch/side-4/mac.welcome/dark/now/more",
        "/designs/bae/scratch/side-4/mac.welcome",
        "/design/bae/live/side-4/mac.welcome",
    ] {
        assert_eq!(route(other), None, "{other}");
    }
}

#[test]
fn a_worktree_is_the_main_checkout_or_a_folder_of_the_repos_worktrees_folder() {
    let scratch = Scratch::new("scratch-worktree");
    let (main, side) = checkouts(&scratch);
    let found = worktree_of(&main, &globs(), "side-1").unwrap();
    let top = std::fs::canonicalize(&side).unwrap();
    assert_eq!(
        found,
        Worktree {
            folders: vec![top.join("design")],
            top,
        }
    );
    let main_top = std::fs::canonicalize(&main).unwrap();
    assert_eq!(worktree_of(&main, &globs(), "main").unwrap().top, main_top);

    for name in ["", ".", "..", "side-1/design", "../bae"] {
        assert!(
            message(worktree_of(&main, &globs(), name)).contains("names no worktree"),
            "{name:?}"
        );
    }
    assert!(message(worktree_of(&main, &globs(), "side-9")).contains("has no worktree side-9"));
    // A folder there that is no worktree of the repo: a plain folder, and
    // another repo's checkout.
    let worktrees = worktrees_folder(&main).unwrap();
    std::fs::create_dir_all(worktrees.join("plain")).unwrap();
    repo(&worktrees.join("other"), "");
    for name in ["plain", "other"] {
        assert!(
            message(worktree_of(&main, &globs(), name)).contains("is no worktree of"),
            "{name}"
        );
    }
}

#[test]
fn the_drawing_gives_each_sides_size_or_why_it_has_none() {
    let scratch = Scratch::new("scratch-drawing");
    let (main, _) = checkouts(&scratch);
    let worktree = worktree_of(&main, &globs(), "side-1").unwrap();
    assert_eq!(
        drawing(&worktree, "A"),
        json!({ "now": { "w": 300, "h": 200 }, "worktree": { "w": 500, "h": 100 } })
    );
    // A board that only the worktree's files hold is new on origin/main.
    assert_eq!(
        drawing(&worktree, "design/mocks/N.dc.html"),
        json!({ "now": { "absent": true }, "worktree": { "w": 40, "h": 30 } })
    );
    let gone = drawing(&worktree, "Gone");
    assert_eq!(gone["now"], json!({ "absent": true }));
    assert!(
        gone["worktree"]["error"]
            .as_str()
            .unwrap()
            .contains("Gone is no file in"),
        "{gone}"
    );
    let bad = drawing(&worktree, "../A");
    assert!(bad["now"]["error"].is_string() && bad["worktree"]["error"].is_string());
    // A scratch board is on no canvas: the page sizes its frame to what it
    // draws.
    assert_eq!(
        drawing(&worktree, "scratch.rows"),
        json!({ "now": { "absent": true }, "worktree": { "fit": true } })
    );
    let gone = drawing(&worktree, "scratch.gone");
    assert!(
        gone["worktree"]["error"]
            .as_str()
            .unwrap()
            .contains("scratch.gone is no file in"),
        "{gone}"
    );
}

#[test]
fn the_scratch_list_names_each_scratch_board_of_each_worktree_of_the_repo() {
    let scratch = Scratch::new("scratch-list");
    let (main, side) = checkouts(&scratch);
    let side_top = std::fs::canonicalize(&side).unwrap();
    let main_top = std::fs::canonicalize(&main).unwrap();
    assert_eq!(
        named_worktrees(&main).unwrap(),
        vec![
            ("main".to_string(), main_top.clone()),
            ("side-1".to_string(), side_top.clone())
        ]
    );
    // The main checkout's scratch boards list first. A file that is no
    // scratch board is left out, and one whose name names no board lists
    // with why.
    let mocks = main.join("design/mocks");
    for file in [
        "scratch.b.dc.html",
        "scratch.a.dc.html",
        "scratch.c.html",
        "scratchy.dc.html",
        "scratch.x y.dc.html",
    ] {
        std::fs::write(mocks.join(file), "<x-dc></x-dc>").unwrap();
    }
    let entry = |worktree: &str, board: &str| {
        json!({
            "worktree": worktree,
            "board": board,
            "link": format!("/design/bae/scratch/{worktree}/{board}"),
        })
    };
    let list = scratch_list("bae", &main).unwrap();
    assert_eq!(list.len(), 4, "{list:?}");
    assert_eq!(
        list[..2],
        [entry("main", "scratch.a"), entry("main", "scratch.b")]
    );
    assert_eq!(list[2]["file"], json!("scratch.x y.dc.html"));
    assert!(
        list[2]["error"]
            .as_str()
            .unwrap()
            .contains("\"scratch.x y\" is no board"),
        "{list:?}"
    );
    assert_eq!(list[3], entry("side-1", "scratch.rows"));
    // A review page lists its change's worktree alone, named by its folder
    // through any link, and refuses a folder that is no worktree of the repo.
    assert_eq!(
        scratch_list_of("bae", &main, &side).unwrap(),
        [entry("side-1", "scratch.rows")]
    );
    assert!(message(scratch_list_of("bae", &main, &scratch.dir)).contains("is no worktree of"),);
    assert!(message(scratch_list_of("bae", &main, &scratch.dir.join("gone"))).contains("reading"));
    // A worktree with no design/mocks has no scratch boards.
    assert!(scratch_files_in(&scratch.dir).unwrap().is_empty());
}

#[test]
fn the_page_names_its_board_and_where_its_frames_and_events_are() {
    let scratch = Scratch::new("scratch-page");
    let (main, _) = checkouts(&scratch);
    let worktree = worktree_of(&main, &globs(), "side-1").unwrap();
    let target = Target {
        project: "bae",
        worktree: "side-1",
        board: "A",
    };
    let html = page(target, &worktree).unwrap();
    assert!(
        html.contains(r#""base":"/design/bae/scratch/side-1/A""#)
            && html.contains(r#""board":"A""#)
            && !html.contains("@@"),
        "{html}"
    );
    let bad = Target {
        board: "../A",
        ..target
    };
    assert!(message(page(bad, &worktree)).contains("\"../A\" is no board"));
}

#[test]
fn a_batch_that_only_says_a_copy_read_files_changes_nothing() {
    let event = |flags| Event {
        path: PathBuf::from("/wt/design/system/"),
        flags,
    };
    let cloned = flags::ITEM_CLONED;
    let modified = 0x1000;
    let created = 0x100;
    assert!(!changes_files(&[event(cloned), event(cloned | 0x1_0000)]));
    assert!(changes_files(&[event(cloned), event(modified | 0x1_0000)]));
    // A file that a copy makes by cloning is new: a change.
    assert!(changes_files(&[event(created | cloned)]));
    // A folder-level event with no item flags, or one that asks for a read
    // of the folder again.
    assert!(changes_files(&[event(0)]));
    assert!(changes_files(&[event(flags::MUST_SCAN_SUBDIRS)]));
}
