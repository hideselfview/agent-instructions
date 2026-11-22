use super::*;
use crate::lists::Resolution;
use crate::lists::fixtures::{NOON, design_drawn, design_place, design_tipped, stamp};
use crate::lists::project::NewProject;
use crate::lists::store::Store;
use crate::testing::{git, repo};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BOARD: &str = "design/mocks/Held.dc.html";

/// Makes a claimed side checkout whose board has different bytes from the
/// open record. The real design reader must ask its renderer about coverage.
fn checkout_with_redrawn_board(scratch: &Scratch, places: &crate::places::Places) {
    let main = scratch.repo_path("bae");
    repo(&main, "");
    std::fs::create_dir_all(main.join("design/mocks")).unwrap();
    std::fs::write(main.join(BOARD), "before").unwrap();
    std::fs::write(
        main.join("design/mocks/canvas.json"),
        r#"{"designSystems": [{"namespace": "demo"}], "boards": {}}"#,
    )
    .unwrap();
    git(&main, &["add", "design"]);
    git(&main, &["commit", "-q", "-m", "Draw the board"]);
    git(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let recorded = crate::lists::design_paths::full_sha(&main, "HEAD").unwrap();
    let side = main.parent().unwrap().join("bae-worktrees/side-2");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "redraw",
            side.to_str().unwrap(),
        ],
    );
    std::fs::write(side.join(BOARD), "after").unwrap();
    git(&side, &["commit", "-qam", "Redraw the board"]);
    let checkout = crate::checkout::Checkout::find(side.to_str().unwrap()).unwrap();
    let tool = scratch.tool(0);
    crate::setup::setup(&tool, &crate::testing::no_ssh(), &checkout).unwrap();
    crate::claim::write_claim(&checkout, "designer", tool.now, None).unwrap();

    let store = Store::named(places, "demo").unwrap();
    store
        .create(
            &NewProject {
                name: "demo".into(),
                canvas: true,
                repos: vec!["bae".into()],
                places: std::collections::BTreeMap::from([(
                    "bae".into(),
                    vec!["design/mocks/*.dc.html".into()],
                )]),
                sources: Default::default(),
                specs: Default::default(),
            },
            &stamp(NOON),
            "lists init demo",
        )
        .unwrap();
    store
        .write(&stamp(NOON), "design record", |db| {
            let id = design_drawn(db, side.to_str().unwrap(), &[], None);
            design_tipped(db, id, &recorded);
            design_place(db, id, BOARD, Resolution::Matches("checked".into()));
            Ok(())
        })
        .unwrap();
}

#[test]
fn checkout_lists_and_health_answer_before_a_board_comparison_finishes() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (entered, drawing) = mpsc::channel();
    std::thread::spawn(move || {
        let (gate, _) = listener.accept().unwrap();
        entered.send(gate).unwrap();
    });
    let server = server_with("http-pending-design", |scratch, places| {
        checkout_with_redrawn_board(scratch, places);
        scratch.script(
            "states-shown",
            &format!(
                r#"test "$2" = draws || exit 0
exec /usr/bin/python3 -c '
import socket
with socket.create_connection(("127.0.0.1", {port}), timeout=8) as gate:
    assert gate.recv(1) == b"x"
print("alike board Held")
'"#
            ),
        );
    });
    let mut gate = drawing.recv_timeout(Duration::from_secs(5)).unwrap();
    server.model.refresh_processes().unwrap();
    let started = Instant::now();
    let checkout = server.get("/api/state");
    let lists = server.get("/api/lists");
    let health = server.get("/api/health");
    gate.write_all(b"x").unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "render was still blocked"
    );
    assert_eq!(checkout.0, 200, "{}", checkout.2);
    assert!(checkout.2.contains("side-2"), "{}", checkout.2);
    assert_eq!(lists.0, 200, "{}", lists.2);
    assert!(lists.2.contains("checking-design"), "{}", lists.2);
    assert_eq!(health.0, 200, "{}", health.2);
    assert!(
        health.2.contains("checking design coverage"),
        "{}",
        health.2
    );
}
