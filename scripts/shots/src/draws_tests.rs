use super::*;
use crate::shots::tests::png_of;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ran = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(ran.status.success(), "git {args:?}: {ran:?}");
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The checker's cache folder, and no fonts folder, as on a host that is not
/// a Mac.
fn folders() -> boards::Folders {
    boards::Folders {
        cache: crate::render::fixture::cache(),
        fonts: None,
        volume: crate::render::fixture::volume(),
    }
}

const BOARD: &str = "dashboard.designs-publish";

/// The repo's own design files as commit `a`, then `b`, which changes bytes
/// only (a board position in canvas.json and a comment in a board's file), and
/// `c`, which changes the board's padding.
fn history() -> tempfile::TempDir {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../design");
    let repo = tempfile::tempdir().unwrap();
    copy_tree(&here, &repo.path().join("design"));
    // The design system the boards load, which the repo's own canvas.json
    // does not name.
    let canvas = repo.path().join("design/mocks/canvas.json");
    let text = std::fs::read_to_string(&canvas).unwrap();
    let text = text.replacen(
        "\"attachments\": {},",
        "\"attachments\": {}, \"designSystems\": [{\"namespace\": \"demo\"}],",
        1,
    );
    std::fs::write(&canvas, text).unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "a"]);
    git(repo.path(), &["tag", "a"]);
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&canvas).unwrap()).unwrap();
    index["boards"][format!("{BOARD}{}", boards::BOARD_SUFFIX)]["x"] = 9000.into();
    std::fs::write(&canvas, serde_json::to_vec(&index).unwrap()).unwrap();
    let board = repo.path().join(boards::board_path(BOARD));
    let html = std::fs::read_to_string(&board).unwrap();
    std::fs::write(&board, format!("{html}\n<!-- merged from main -->\n")).unwrap();
    git(repo.path(), &["commit", "-q", "-am", "b"]);
    git(repo.path(), &["tag", "b"]);
    let html = std::fs::read_to_string(&board).unwrap();
    assert!(html.contains("padding:24px"));
    std::fs::write(&board, html.replacen("padding:24px", "padding:40px", 1)).unwrap();
    git(repo.path(), &["commit", "-q", "-am", "c"]);
    git(repo.path(), &["tag", "c"]);
    repo
}

#[test]
fn a_board_whose_bytes_change_and_drawing_does_not_draws_alike_and_a_redrawn_one_differs() {
    let repo = history();
    let boards = vec![BOARD.to_string()];
    let same = lines(repo.path(), "a", "b", &boards, &[], &folders(), None).unwrap();
    assert_eq!(same, vec![format!("alike board {BOARD}")]);
    let moved = lines(repo.path(), "b", "c", &boards, &[], &folders(), None).unwrap();
    assert_eq!(moved, vec![format!("differs board {BOARD}")]);
}

#[test]
fn a_board_draws_at_the_device_scale_of_a_shot() {
    let repo = history();
    let drawn = read(
        repo.path(),
        "a",
        &folders(),
        Selection::Named {
            boards: &[BOARD.to_string()],
            components: &[],
        },
        None,
    )
    .unwrap();
    let picture = decode(&drawn[&format!("board {BOARD}")].as_ref().unwrap().png).unwrap();
    let frame = design::frames(repo.path()).unwrap()[BOARD];
    assert_eq!(
        (picture.width, picture.height),
        (frame.w * 2, frame.h * 2),
        "the picture holds 2 pixels to the point"
    );
}

#[test]
fn a_preview_draws_at_rest_even_when_a_rule_marks_an_animation_important() {
    let repo = history();
    let component = "Welcome";
    let path = boards::preview::preview_path(component);
    let preview = repo.path().join(&path);
    std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
    for (revision, stylesheet, inline) in [
        ("at-rest", "", ""),
        (
            "stylesheet-bob",
            ".logo { animation: bob 3s ease-in-out infinite !important }",
            "",
        ),
        (
            "inline-bob",
            "",
            "animation: bob 3s ease-in-out infinite !important",
        ),
    ] {
        std::fs::write(
            &preview,
            format!(
                "<!doctype html><html><head><style>\
                 @keyframes bob {{ from {{ translate: 0 24px }} to {{ translate: 0 40px }} }}\
                 {stylesheet}</style></head>\
                 <body><button class='logo' style='{inline}'>Welcome</button>\
                 <script>\
                 for (const animation of document.getAnimations()) {{\
                   animation.finished.catch(() => {{\
                     document.querySelector('button').textContent = 'Animation interrupted';\
                   }});\
                 }}\
                 </script></body></html>"
            ),
        )
        .unwrap();
        git(repo.path(), &["add", "--", &path]);
        git(repo.path(), &["commit", "-q", "-m", revision]);
        git(repo.path(), &["tag", revision]);
    }
    for revision in ["stylesheet-bob", "inline-bob"] {
        let drawings = lines(
            repo.path(),
            revision,
            "at-rest",
            &[],
            &[component.to_string()],
            &folders(),
            None,
        )
        .unwrap();
        assert_eq!(drawings, ["alike component Welcome"], "{revision}");
    }
}

#[test]
fn a_label_below_the_viewport_changes_the_full_preview_drawing() {
    let repo = history();
    let component = "MacRow";
    let path = boards::preview::preview_path(component);
    let preview = repo.path().join(&path);
    std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
    for (revision, label) in [
        ("before-label", "Open album"),
        ("after-label", "Open artist"),
    ] {
        std::fs::write(
            &preview,
            format!("<html><head></head><body><button style='margin-top:1200px' aria-label='{label}'>Open</button></body></html>"),
        )
        .unwrap();
        git(repo.path(), &["add", "--", &path]);
        git(repo.path(), &["commit", "-q", "-m", revision]);
        git(repo.path(), &["tag", revision]);
    }
    let drawings = lines(
        repo.path(),
        "before-label",
        "after-label",
        &[],
        &[component.to_string()],
        &folders(),
        None,
    )
    .unwrap();
    assert_eq!(drawings, ["differs component MacRow"]);
}

/// `history()` plus commit `p`, which adds the preview of a component, and
/// commit `d`, which deletes that preview and the board's file, and commit
/// `e`, which deletes them on a branch from `p`.
fn history_with_deletions() -> (tempfile::TempDir, String) {
    let repo = history();
    let component = "MacRow".to_string();
    let preview = repo
        .path()
        .join(boards::preview::COMPONENTS)
        .join(&component)
        .join("preview.html");
    std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
    std::fs::write(
        &preview,
        "<html><head></head><body><p>a row</p></body></html>",
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "p"]);
    git(repo.path(), &["tag", "p"]);
    std::fs::remove_file(&preview).unwrap();
    delete_board(repo.path());
    git(repo.path(), &["commit", "-q", "-am", "d"]);
    git(repo.path(), &["tag", "d"]);
    // `e` also deletes both, on a branch of its own from `p`.
    git(repo.path(), &["checkout", "-q", "-b", "other", "p"]);
    std::fs::remove_file(&preview).unwrap();
    delete_board(repo.path());
    std::fs::write(repo.path().join("marker"), "e").unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "e"]);
    git(repo.path(), &["tag", "e"]);
    (repo, component)
}

fn delete_board(repo: &Path) {
    std::fs::remove_file(repo.join(boards::board_path(BOARD))).unwrap();
    let canvas = repo.join(boards::CANVAS);
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&canvas).unwrap()).unwrap();
    index["boards"]
        .as_object_mut()
        .unwrap()
        .remove(&format!("{BOARD}{}", boards::BOARD_SUFFIX))
        .unwrap();
    std::fs::write(canvas, serde_json::to_vec(&index).unwrap()).unwrap();
}

#[test]
fn a_board_or_component_that_neither_revision_draws_draws_alike_when_their_merge_base_holds_it() {
    let (repo, component) = history_with_deletions();
    let boards = vec![BOARD.to_string()];
    let components = vec![component.clone()];
    let both_gone = lines(
        repo.path(),
        "d",
        "e",
        &boards,
        &components,
        &folders(),
        None,
    )
    .unwrap();
    assert_eq!(
        both_gone,
        vec![
            format!("alike board {BOARD}"),
            format!("alike component {component}"),
        ]
    );
}

#[test]
fn a_board_or_component_that_no_revision_holds_is_an_error() {
    let (repo, _) = history_with_deletions();
    let missing = vec!["no-such-board".to_string()];
    let error = lines(repo.path(), "d", "e", &missing, &[], &folders(), None).unwrap_err();
    assert!(
        error.contains("board no-such-board exists at neither"),
        "{error}"
    );
    let missing = vec!["NoSuchComponent".to_string()];
    let error = lines(repo.path(), "d", "e", &[], &missing, &folders(), None).unwrap_err();
    assert!(
        error.contains("component NoSuchComponent exists at neither"),
        "{error}"
    );
}

#[test]
fn a_board_or_component_that_exists_at_one_revision_only_differs() {
    let (repo, component) = history_with_deletions();
    let boards = vec![BOARD.to_string()];
    let components = vec![component.clone()];
    for (at, against) in [("p", "d"), ("d", "p")] {
        let changed = lines(
            repo.path(),
            at,
            against,
            &boards,
            &components,
            &folders(),
            None,
        )
        .unwrap();
        assert_eq!(
            changed,
            vec![
                format!("differs board {BOARD}"),
                format!("differs component {component}"),
            ],
            "{at} against {against}"
        );
    }
}

#[test]
fn without_selectors_draws_compares_the_boards_and_previews_of_both_revisions() {
    let (repo, component) = history_with_deletions();
    for (at, against) in [("p", "d"), ("d", "p")] {
        let drawings = lines(repo.path(), at, against, &[], &[], &folders(), None).unwrap();
        assert!(
            drawings.contains(&format!("differs board {BOARD}")),
            "{drawings:?}"
        );
        assert!(
            drawings.contains(&format!("differs component {component}")),
            "{drawings:?}"
        );
        assert!(
            drawings.iter().any(|line| line.starts_with("alike board ")),
            "{drawings:?}"
        );
    }
}

#[test]
fn without_selectors_draws_reports_a_canvas_entry_whose_file_is_missing() {
    let repo = history();
    std::fs::remove_file(repo.path().join(boards::board_path(BOARD))).unwrap();
    git(repo.path(), &["commit", "-q", "-am", "missing board"]);
    let error = lines(repo.path(), "HEAD", "c", &[], &[], &folders(), None).unwrap_err();
    assert!(
        error.starts_with("HEAD:") && error.contains(&boards::board_path(BOARD)),
        "{error}"
    );
}

fn drawing(png: &[u8]) -> Picture {
    Picture::of(png, &[]).unwrap()
}

#[test]
fn pictures_whose_pixels_differ_by_the_renderers_noise_draw_alike() {
    let base = png_of(40, 30, |_, _| [90, 120, 200, 255]);
    let noisy = png_of(40, 30, |x, y| {
        if (x + y) % 7 == 0 {
            [92, 118, 202, 255]
        } else {
            [90, 120, 200, 255]
        }
    });
    assert_ne!(base, noisy);
    assert!(drawing(&base).draws_like(&drawing(&noisy)).unwrap());
}

#[test]
fn pictures_with_a_pixel_further_apart_than_the_noise_or_another_size_differ() {
    let base = png_of(40, 30, |_, _| [90, 120, 200, 255]);
    let recoloured = png_of(40, 30, |x, y| {
        if (x, y) == (3, 4) {
            [93, 120, 200, 255]
        } else {
            [90, 120, 200, 255]
        }
    });
    assert!(!drawing(&base).draws_like(&drawing(&recoloured)).unwrap());
    let taller = png_of(40, 31, |_, _| [90, 120, 200, 255]);
    assert!(!drawing(&base).draws_like(&drawing(&taller)).unwrap());
}

#[test]
fn drawings_with_other_element_lists_differ_whatever_their_pictures() {
    let base = png_of(4, 4, |_, _| [1, 2, 3, 255]);
    let one = Picture {
        png: base.clone(),
        elements: "[{\"label\":\"Play\"}]".to_string(),
    };
    let two = Picture {
        png: base,
        elements: "[{\"label\":\"Pause\"}]".to_string(),
    };
    assert!(!one.draws_like(&two).unwrap());
}

const SHOT_BOARD: &str = "mac.dash.board";

/// `history()`'s repo with the board also under a shot name: commit `s1`, then
/// `s2`, which adds a comment to the file and so changes the board hash.
fn history_with_shot_board() -> tempfile::TempDir {
    let repo = history();
    let canvas = repo.path().join("design/mocks/canvas.json");
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&canvas).unwrap()).unwrap();
    index["boards"][format!("{SHOT_BOARD}{}", boards::BOARD_SUFFIX)] =
        index["boards"][format!("{BOARD}{}", boards::BOARD_SUFFIX)].clone();
    std::fs::write(&canvas, serde_json::to_vec(&index).unwrap()).unwrap();
    let board = repo.path().join(boards::board_path(SHOT_BOARD));
    std::fs::copy(repo.path().join(boards::board_path(BOARD)), &board).unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "s1"]);
    git(repo.path(), &["tag", "s1"]);
    let html = std::fs::read_to_string(&board).unwrap();
    std::fs::write(&board, format!("{html}\n<!-- a comment -->\n")).unwrap();
    git(repo.path(), &["commit", "-q", "-am", "s2"]);
    git(repo.path(), &["tag", "s2"]);
    repo
}

#[test]
fn a_screen_board_is_drawn_once_and_then_taken_from_the_picture_folder_by_its_hash() {
    let repo = history_with_shot_board();
    let folder = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(folder.path().to_path_buf(), std::time::SystemTime::now());
    let store = Store {
        pictures: &pictures,
        capture_version: "c0c0c0c0c0c0c0c0",
    };
    let boards = vec![SHOT_BOARD.to_string()];
    let first = lines(
        repo.path(),
        "s1",
        "s1",
        &boards,
        &[],
        &folders(),
        Some(&store),
    )
    .unwrap();
    assert_eq!(first, vec![format!("alike board {SHOT_BOARD}")]);
    let kept: Vec<_> = std::fs::read_dir(folder.path().join("boards"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert_eq!(kept.len(), 1, "the one drawing of the one hash is kept");
    // A kept list that differs from a fresh drawing shows which one the
    // comparison used: s1 comes from the folder, s2 (another hash) is drawn.
    let mut list: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&kept[0]).unwrap()).unwrap();
    list["elements"].as_array_mut().unwrap().pop();
    std::fs::write(&kept[0], serde_json::to_vec(&list).unwrap()).unwrap();
    let second = lines(
        repo.path(),
        "s1",
        "s2",
        &boards,
        &[],
        &folders(),
        Some(&store),
    )
    .unwrap();
    assert_eq!(second, vec![format!("differs board {SHOT_BOARD}")]);
    let unkept = lines(repo.path(), "s1", "s2", &boards, &[], &folders(), None).unwrap();
    assert_eq!(unkept, vec![format!("alike board {SHOT_BOARD}")]);
}
