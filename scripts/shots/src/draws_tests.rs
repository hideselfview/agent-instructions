use super::*;
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

fn cache() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
        .join(".cache/builds-shots")
}

const BOARD: &str = "dashboard.designs-publish";

/// The repo's own design files as commit `a`, then `b`, which changes bytes
/// only (an extra entry in canvas.json and a comment in a board's file), and
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
    let text = std::fs::read_to_string(&canvas).unwrap();
    let text = text.replacen(
        "\"boards\": {",
        "\"boards\": {\n  \"unrelated.dc.html\": {\"page\": \"dashboard\", \"title\": \"U\", \"x\": 0, \"y\": 9000, \"w\": 10, \"h\": 10},",
        1,
    );
    std::fs::write(&canvas, text).unwrap();
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
    let same = lines(repo.path(), "a", "b", &boards, &[], &cache()).unwrap();
    assert_eq!(same, vec![format!("alike board {BOARD}")]);
    let moved = lines(repo.path(), "b", "c", &boards, &[], &cache()).unwrap();
    assert_eq!(moved, vec![format!("differs board {BOARD}")]);
}

/// `history()` plus commit `p`, which adds the preview of a component, and
/// commit `d`, which deletes that preview and the board's file.
fn history_with_deletions() -> (tempfile::TempDir, String) {
    let repo = history();
    let component = "MacRow".to_string();
    let preview = repo
        .path()
        .join(boards::preview::COMPONENTS)
        .join(&component)
        .join("preview.html");
    std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
    std::fs::write(&preview, "<body><p>a row</p></body>").unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "p"]);
    git(repo.path(), &["tag", "p"]);
    std::fs::remove_file(preview).unwrap();
    std::fs::remove_file(repo.path().join(boards::board_path(BOARD))).unwrap();
    git(repo.path(), &["commit", "-q", "-am", "d"]);
    git(repo.path(), &["tag", "d"]);
    (repo, component)
}

#[test]
fn a_board_or_component_that_exists_at_neither_revision_draws_alike() {
    let (repo, component) = history_with_deletions();
    let boards = vec!["no-such-board".to_string(), BOARD.to_string()];
    let components = vec!["NoSuchComponent".to_string(), component.clone()];
    let both_gone = lines(repo.path(), "d", "d", &boards, &components, &cache()).unwrap();
    assert_eq!(
        both_gone,
        vec![
            format!("alike board {BOARD}"),
            "alike board no-such-board".to_string(),
            format!("alike component {component}"),
            "alike component NoSuchComponent".to_string(),
        ]
    );
}

#[test]
fn a_board_or_component_that_exists_at_one_revision_only_differs() {
    let (repo, component) = history_with_deletions();
    let boards = vec![BOARD.to_string()];
    let components = vec![component.clone()];
    for (at, against) in [("p", "d"), ("d", "p")] {
        let changed = lines(repo.path(), at, against, &boards, &components, &cache()).unwrap();
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
