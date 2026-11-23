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
    let pid = std::process::id();
    let (status, reply) = server.post_to(
        "/api/queue",
        &host,
        Some(&format!("http://{host}")),
        &format!(r#"{{"action":"top","repo":"demo","pid":{pid}}}"#),
    );
    let name = ticket.name.clone();
    drop(ticket);
    line.drop_ticket(&name).unwrap();
    assert_eq!(status, 200, "{reply}");

    let health = server.get("/api/health");
    assert_eq!(health.0, 200, "{}", health.2);
    let data: serde_json::Value = serde_json::from_str(&health.2).unwrap();
    // Health also lists findings about this machine's real processes. The
    // design comparison only produces findings of the lists kind.
    let from_lists: Vec<&serde_json::Value> = data["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["kind"] == "lists")
        .collect();
    assert!(from_lists.is_empty(), "{from_lists:?}");
    assert_eq!(data["worktrees"][0]["idle_for_release"], false);

    gate.write_all(b"x").unwrap();
}

#[test]
fn the_checkout_stream_answers_while_git_reads_a_review_verdict() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (entered, reading) = mpsc::channel();
    std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        entered.send(socket).unwrap();
    });
    let s = server_with("http-pending-rules-review", |scratch, places| {
        let fake = crate::review::fakes::fake(&scratch.dir);
        let installed = crate::review::rules::installed_root(&places.home);
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(fake.root, installed).unwrap();
        crate::lists::fixtures::make_store(places, &["demo"]);
        let main = scratch.repo_path("demo");
        repo(&main, "");
        std::fs::write(main.join(".gitattributes"), "a.rs diff=dashboard\n").unwrap();
        std::fs::write(main.join("a.rs"), "const N: u32 = 1;\n").unwrap();
        git(&main, &["add", "--", ".gitattributes", "a.rs"]);
        git(&main, &["commit", "-qm", "Add a constant"]);
        git(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let side = places.home.join("dev/demo-worktrees/side-2");
        git(
            &main,
            &["worktree", "add", "-qb", "work", side.to_str().unwrap()],
        );
        std::fs::write(side.join("a.rs"), "const N: u32 = 2;\n").unwrap();
        git(&side, &["commit", "-qam", "Change the constant"]);
        let checkout = crate::checkout::Checkout::find(side.to_str().unwrap()).unwrap();
        crate::setup::setup(&scratch.tool(0), &crate::testing::no_ssh(), &checkout).unwrap();

        // The real Git diff invokes the driver before it can return its text.
        let driver = scratch.script(
            "review-diff",
            &format!(
                r#"if [ ! -e '{}/finish-review-diff' ]; then
  /usr/bin/python3 -c '
import socket
with socket.create_connection(("127.0.0.1", {port}), timeout=10) as gate:
    assert gate.recv(1) == b"x"
'
  exit 1
fi
cat "$1""#,
                scratch.dir.display()
            ),
        );
        git(
            &side,
            &[
                "config",
                "diff.dashboard.textconv",
                &format!("'{}'", driver.display()),
            ],
        );
    });
    let host = format!("127.0.0.1:{}", s.port);
    let open = || {
        let mut stream = TcpStream::connect(&host).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(stream, "GET /events HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
        BufReader::new(stream)
    };
    let wait_for = |events: &mut BufReader<TcpStream>, text: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = next_event(events, "state");
            if state.contains(text) {
                break;
            }
            assert!(Instant::now() < deadline, "{state}");
        }
    };
    let mut events = open();
    wait_for(&mut events, "Review status");
    let mut gate = reading.recv_timeout(Duration::from_secs(10)).unwrap();
    let state = next_event(&mut open(), "state");
    assert!(state.contains("Review status"), "{state}");
    std::fs::write(s.scratch.dir.join("finish-review-diff"), "").unwrap();
    gate.write_all(b"x").unwrap();
    wait_for(&mut events, "The daemon can't read the rules review");
    wait_for(&mut events, "Needs review");
}
