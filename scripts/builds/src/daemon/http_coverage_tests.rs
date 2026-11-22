use super::*;
use crate::lists::Resolution;
use crate::lists::fixtures::{NOON, design_drawn, design_place, design_tipped, stamp};
use crate::lists::project::NewProject;
use crate::lists::store::Store;
use crate::testing::{git, repo};
use std::os::fd::FromRawFd;
use std::sync::mpsc;
use std::time::Duration;

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
    gate.settimeout(None)
    assert gate.recv(1) == b"x"
print("alike board Held")
'"#
            ),
        );
    });
    let mut gate = drawing.recv_timeout(Duration::from_secs(5)).unwrap();
    server.model.refresh_processes().unwrap();
    let host = format!("127.0.0.1:{}", server.port);
    let mut stream = TcpStream::connect(&host).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(stream, "GET /events HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
    let mut events = BufReader::new(stream);
    assert!(next_event(&mut events, "state").contains("side-2"));
    assert!(next_event(&mut events, "lists").contains("checking-design"));

    let places = &server.model.places;
    let line = crate::queue::Queue::of(places, "demo", crate::lane::Lane::Mac);
    let (ticket, _) = line.join(crate::lock::Where::Anywhere).unwrap();
    let ticket_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(ticket.fd) };
    let pid = std::process::id();
    let (status, reply) = server.post_to(
        "/api/queue",
        &host,
        Some(&format!("http://{host}")),
        &format!(r#"{{"action":"top","repo":"demo","pid":{pid}}}"#),
    );
    drop(ticket_fd);
    line.drop_ticket(&ticket.name).unwrap();
    assert_eq!(status, 200, "{reply}");

    let health = server.get("/api/health");
    assert_eq!(health.0, 200, "{}", health.2);
    let data: serde_json::Value = serde_json::from_str(&health.2).unwrap();
    // Health also reads this machine's real processes, so it may list
    // findings about them; none may concern the checkout under comparison.
    let about_the_checkout: Vec<&serde_json::Value> = data["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["text"].as_str().unwrap().contains("side-2"))
        .collect();
    assert!(about_the_checkout.is_empty(), "{about_the_checkout:?}");
    assert_eq!(data["worktrees"][0]["stalled"], false);

    gate.write_all(b"x").unwrap();
}
