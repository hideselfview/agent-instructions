use super::*;
use crate::daemon::runs::{Phase, RunPage, Runs};
use crate::daemon::tracker::Tracker;
use crate::testing::Scratch;
use std::net::TcpListener;

/// A daemon's page server on a port of its own, over a scratch volume whose
/// `demo` repo has a mac app checkout. Its build-app is a shell script that
/// waits for the `gate` file, then ends with status 3.
struct Server {
    scratch: Scratch,
    port: u16,
    model: Arc<Model>,
}

const WAIT_THEN_FAIL: &str = "while [ ! -e \"$0\" ]; do sleep 0.05; done; echo \"ran: $*\"; exit 3";

fn server(name: &str) -> Server {
    server_with(name, |_, _| {})
}

/// `server`, after <prepare> has made what the daemon reads at start (the
/// scratch volume's lists, say) in the scratch folder and the places.
fn server_with(name: &str, prepare: impl FnOnce(&Scratch, &crate::places::Places)) -> Server {
    let scratch = Scratch::new(name);
    let tool = scratch.tool(0);
    let places = tool.places.clone();
    prepare(&scratch, &places);
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.root.join("demo/app-checkouts"), "side-1\n").unwrap();
    let root = std::fs::canonicalize(&places.root).unwrap();
    let tracker = Tracker::start(&root, &places.daemon_dir()).unwrap();
    let gate = scratch.dir.join("gate");
    let program = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        WAIT_THEN_FAIL.to_string(),
        gate.to_string_lossy().into_owned(),
    ];
    let runs = Runs::start(places.clone(), program).unwrap();
    let model = Model::start(places, tool.rules.clone(), tracker, runs);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = model.clone();
    let names: Arc<[String]> = vec![
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("{ALSO}:{port}"),
    ]
    .into();
    std::thread::spawn(move || serve(listener, names, serving));
    Server {
        scratch,
        port,
        model,
    }
}

impl Server {
    /// The status code and body of a POST to /api/build-app.
    fn post(&self, host: &str, origin: Option<&str>, body: &str) -> (u16, String) {
        let origin = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /api/build-app HTTP/1.1\r\nHost: {host}\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let status = reply.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, reply.split_once("\r\n\r\n").unwrap().1.to_string())
    }

    /// The status code and body of a POST of <body> to <path>.
    fn post_to(&self, path: &str, host: &str, origin: Option<&str>, body: &str) -> (u16, String) {
        let origin = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let status = reply.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, reply.split_once("\r\n\r\n").unwrap().1.to_string())
    }

    fn own(&self, body: &str) -> (u16, String) {
        let origin = format!("http://127.0.0.1:{}", self.port);
        self.post(&format!("127.0.0.1:{}", self.port), Some(&origin), body)
    }

    fn run(&self) -> Option<RunPage> {
        self.model
            .runs
            .pages("demo", &|_| Vec::new())
            .into_iter()
            .next()
    }
}

const MAC: &str = r#"{"repo":"demo","lane":"mac","ref":"my-branch"}"#;

/// The other address the test server answers for, as `--listen` gives one.
const ALSO: &str = "192.0.2.7";

#[test]
fn the_page_at_an_address_it_also_listens_on_changes_nothing() {
    let s = server("http-also");
    let also = format!("{ALSO}:{}", s.port);
    let origin = format!("http://{also}");
    let local = format!("127.0.0.1:{}", s.port);
    let local_origin = format!("http://{local}");
    // Every route that changes state is only on this Mac: the other address's
    // own host and origin pass the header checks and are still refused.
    for (path, body) in [("/api/build-app", MAC), ("/api/queue", "{}")] {
        for (host, origin) in [
            (also.as_str(), Some(origin.as_str())),
            (also.as_str(), Some(local_origin.as_str())),
            (local.as_str(), Some(origin.as_str())),
            (&format!("192.0.2.8:{}", s.port), Some(origin.as_str())),
        ] {
            let (status, reply) = s.post_to(path, host, origin, body);
            assert_eq!(status, 403, "{path} {host} {origin:?}");
            if host == also || host == local {
                assert!(reply.contains("only on this Mac"), "{reply}");
            }
        }
    }
    assert!(s.run().is_none(), "no build started");
    // From this Mac the same routes get past the check (a bad queue request
    // is then its own error).
    let (status, reply) = s.post_to("/api/queue", &local, Some(&local_origin), "{}");
    assert_ne!(status, 403, "{reply}");
    assert!(reply.contains("the request"), "{reply}");
    let localhost = format!("localhost:{}", s.port);
    let (status, _) = s.post_to(
        "/api/queue",
        &localhost,
        Some(&format!("http://{localhost}")),
        "{}",
    );
    assert_ne!(status, 403, "localhost is this Mac too");
}

#[test]
fn a_cross_site_post_starts_no_build() {
    let s = server("http-cross-site");
    let host = format!("127.0.0.1:{}", s.port);
    assert_eq!(
        s.post(&host, Some("https://evil.example"), MAC).0,
        403,
        "another site's origin"
    );
    assert_eq!(
        s.post(&host, Some("null"), MAC).0,
        403,
        "a sandboxed page's origin"
    );
    assert_eq!(
        s.post(
            &host,
            Some(&format!("http://127.0.0.1:{}", s.port + 1)),
            MAC
        )
        .0,
        403,
        "another local port"
    );
    assert_eq!(s.post(&host, None, MAC).0, 403, "no origin at all");
    let origin = format!("http://127.0.0.1:{}", s.port);
    assert_eq!(
        s.post("evil.example", Some(&origin), MAC).0,
        403,
        "a name that resolves here"
    );
    assert!(s.run().is_none(), "none of them started a build");
}

#[test]
fn the_page_starts_one_build_per_lane_and_a_failed_one_shows_failed_with_its_output() {
    let s = server("http-start");
    let (status, body) = s.own(MAC);
    assert_eq!((status, body.as_str()), (200, "{}"));
    let (status, body) = s.own(MAC);
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("already queued or running"), "{body}");
    assert_eq!(s.own(r#"{"repo":"demo","lane":"ios"}"#).0, 400);
    assert_eq!(s.own("not json").0, 400);

    std::fs::write(s.scratch.dir.join("gate"), "").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let failed = loop {
        let run = s.run().unwrap();
        if run.phase == Phase::Failed {
            break run;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the run ended in time"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(failed.exit_code, Some(3));
    assert!(
        failed.output.contains("ran: build-app demo mac my-branch"),
        "{}",
        failed.output
    );
}

impl Server {
    /// The status code, content type and body of a GET of <path> from the machine's own browser.
    fn get(&self, path: &str) -> (u16, String, String) {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            self.port
        );
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let (head, body) = reply.split_once("\r\n\r\n").unwrap();
        let kind = head
            .lines()
            .find_map(|l| l.strip_prefix("Content-Type: "))
            .unwrap_or("")
            .to_string();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            kind,
            body.to_string(),
        )
    }
}

/// A store of project `demo` holding open design change 1, whose worktree is
/// `wt` in the scratch folder, a repo with a commit holding a board `A` and
/// the design system's files, which is the change's last commit. Its review
/// page has two boards, `A` and one that is no file.
fn design_store(scratch: &Scratch, places: &crate::places::Places) {
    use crate::lists::design::ReviewSection;
    use crate::lists::fixtures::{NOON, design_drawn, make_store, stamp};
    use crate::lists::{ReviewBoard, Theme};
    let wt = scratch.dir.join("wt");
    std::fs::create_dir_all(wt.join("design/mocks")).unwrap();
    std::fs::create_dir_all(wt.join("design/system/components")).unwrap();
    std::fs::write(wt.join("design/system/tokens.css"), ":root{}").unwrap();
    std::fs::write(wt.join("design/system/components/bundle.css"), "").unwrap();
    std::fs::write(
        wt.join("design/system/components/bundle.js"),
        "window.Bae = {};",
    )
    .unwrap();
    std::fs::write(
        wt.join("design/mocks/A.dc.html"),
        "<x-dc><p>board A as drawn {{yes}}</p></x-dc>",
    )
    .unwrap();
    std::fs::write(
        wt.join("design/mocks/canvas.json"),
        r#"{"boards": {"A.dc.html": {"w": 300, "h": 200}}}"#,
    )
    .unwrap();
    crate::testing::repo(&wt, "");
    crate::testing::git(&wt, &["add", "design"]);
    crate::testing::git(&wt, &["commit", "-q", "-m", "boards"]);
    crate::testing::git(&wt, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let tip = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&wt)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let store = make_store(places, &["bae", "coven"]);
    store
        .write(&stamp(NOON), "design record", |db| {
            let id = design_drawn(
                db,
                &wt.display().to_string(),
                &[],
                vec![ReviewSection {
                    heading: "The change".into(),
                    words: "what it shows".into(),
                    boards: vec![
                        ReviewBoard {
                            board: "design/mocks/A.dc.html".into(),
                            caption: "A".into(),
                            theme: Theme::Light,
                        },
                        ReviewBoard {
                            board: "design/mocks/Gone.dc.html".into(),
                            caption: "Gone".into(),
                            theme: Theme::Dark,
                        },
                    ],
                }],
            );
            crate::lists::fixtures::design_tipped(db, id, &tip);
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_daemon_draws_a_design_changes_review_page_and_its_boards_from_the_changes_last_commit_on_each_request()
 {
    let s = server_with("http-design", design_store);
    let (status, kind, page) = s.get("/design/1");
    assert_eq!((status, kind.as_str()), (200, "text/html; charset=utf-8"));
    assert!(
        page.contains("\"heading\":\"The change\"") && page.contains("/design/1/board/0"),
        "{page}"
    );
    assert!(page.contains("/design/1/board/0/now") && page.contains("/design/1/board/0/proposed"));
    let (status, kind, board) = s.get("/design/1/board/0/proposed");
    assert_eq!((status, kind.as_str()), (200, "text/html; charset=utf-8"));
    assert!(
        board.contains("board A as drawn") && board.contains("data-theme=\"light\""),
        "{board}"
    );

    // What the worktree has checked out is not what the change draws: a file edited there, and
    // another branch checked out there, leave the board as the change's last commit has it.
    let wt = s.scratch.dir.join("wt");
    let file = wt.join("design/mocks/A.dc.html");
    std::fs::write(&file, "<x-dc><p>board A redrawn</p></x-dc>").unwrap();
    crate::testing::git(&wt, &["checkout", "-q", "-b", "other"]);
    crate::testing::git(&wt, &["commit", "-q", "-a", "-m", "another board"]);
    let (_, _, board) = s.get("/design/1/board/0/proposed");
    assert!(
        board.contains("board A as drawn") && !board.contains("redrawn"),
        "{board}"
    );
    // The now is origin/main, which has moved on to that other board.
    assert!(
        s.get("/design/1/board/0/now")
            .2
            .contains("board A as drawn")
    );
    crate::testing::git(&wt, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let (_, _, now) = s.get("/design/1/board/0/now");
    assert!(now.contains("board A redrawn"), "{now}");

    // A board that is no file is its own problem, said where the frame is.
    let (status, _, problem) = s.get("/design/1/board/1/proposed");
    assert_eq!(status, 500);
    assert!(problem.contains("Gone.dc.html is not there"), "{problem}");
    // No such change, board or route; and the image route is gone.
    for path in [
        "/design/9",
        "/design/1/board/2/now",
        "/design/1/board/0",
        "/design/1/deleted/0/now",
        "/design/x",
        "/design/1/other/0",
        "/design/1/board/0/1/2",
        "/api/design-render/1/0",
    ] {
        assert_eq!(s.get(path).0, 404, "{path}");
    }
}

#[test]
fn a_design_path_is_a_page_or_a_board_of_a_page() {
    assert_eq!(design_route("7"), Some(DesignRoute::Page(7)));
    assert_eq!(
        design_route("7/board/3/now"),
        Some(DesignRoute::Board(7, 3, Side::Now))
    );
    assert_eq!(
        design_route("7/board/3/proposed"),
        Some(DesignRoute::Board(7, 3, Side::Proposed))
    );
    assert_eq!(
        design_route("7/deleted/2/now"),
        Some(DesignRoute::Deleted(7, 2))
    );
    for bad in [
        "",
        "7/",
        "a",
        "7/board",
        "7/board/-1/now",
        "7/board/3",
        "7/board/3/then",
        "7/deleted/2",
        "7/deleted/2/proposed",
        "7/boards/1",
        "7/board/1/2/3",
    ] {
        assert_eq!(design_route(bad), None, "{bad:?}");
    }
}

/// Project `demo` with three hook runs in a repo of its own: two over the limit and one under it.
fn hook_store(_scratch: &Scratch, places: &crate::places::Places) {
    use crate::checks::Kind;
    use crate::hook_stats::{CheckRecord, DATABASE, Outcome, Run, record};
    let store = crate::lists::fixtures::make_store(places, &["demo"]);
    let run = |at: i64, total_ms: u64, clippy_ms: u64| Run {
        at,
        repo: "demo".into(),
        worktree: "/d/demo".into(),
        branch: Some("main".into()),
        base: None,
        passed: true,
        plan_ms: 1,
        total_ms,
        files: vec![],
        checks: vec![CheckRecord {
            name: "clippy".into(),
            kind: Kind::Check,
            outcome: Outcome::Passed,
            handed: Some((1, 4)),
            ms: Some(clippy_ms),
        }],
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // The first ran yesterday, which the panels leave out.
    for r in [
        run(now - 30 * 3600, 80_000, 70_000),
        run(now - 2 * 3600, 90_000, 85_000),
        run(now - 3600, 20_000, 10_000),
    ] {
        record(&store.dir.join(DATABASE), &r).unwrap();
    }
}

#[test]
fn the_daemon_serves_the_hook_runs_with_their_over_limit_runs() {
    let s = server_with("http-hooks", hook_store);
    let (status, kind, body) = s.get("/api/hooks");
    assert_eq!((status, kind.as_str()), (200, "application/json"));
    let hooks: serde_json::Value = serde_json::from_str(&body).unwrap();
    let repo = &hooks["projects"][0]["repos"][0];
    // Of the two runs in the last 12 hours, one took over the limit.
    assert_eq!(repo["runs"], 2);
    assert_eq!(repo["over_count"], 1);
    assert_eq!(repo["over"][0]["total_ms"], 90_000);
    assert_eq!(s.get("/api/hooks/impact?project=demo").0, 404);
}

impl Server {
    /// The status code and body of a POST with no body to <path>.
    fn post_empty(&self, path: &str, host: &str, origin: Option<&str>) -> (u16, String) {
        let origin = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        let request =
            format!("POST {path} HTTP/1.1\r\nHost: {host}\r\n{origin}Content-Length: 0\r\n\r\n");
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let status = reply.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, reply.split_once("\r\n\r\n").unwrap().1.to_string())
    }
}

#[test]
fn the_approve_button_works_only_from_the_page_at_this_mac_and_tells_the_recording_session_once() {
    use crate::lists::design_session::{Messaging, keep};
    use crate::lists::fixtures::{NOON, stamp};
    let mut inbox = None;
    let s = server_with("http-approve", |scratch, places| {
        design_store(scratch, places);
        let socket = scratch.dir.join("s.sock");
        inbox = Some(crate::testing::FakeInbox::listen(&socket));
        let messaging = Messaging::new(socket.to_str().unwrap(), Some("tok-9c1d")).unwrap();
        crate::lists::store::Store::named(places, "demo")
            .unwrap()
            .write(&stamp(NOON), "design record", |db| {
                keep(db, crate::lists::Id(1), Some(&messaging))
            })
            .unwrap();
    });
    let approved = || {
        let store = crate::lists::store::Store::named(&s.model.places, "demo").unwrap();
        store
            .load()
            .unwrap()
            .design(crate::lists::Id(1))
            .unwrap()
            .approved
            .is_some()
    };
    let local = format!("127.0.0.1:{}", s.port);
    let this_mac = format!("http://{local}");
    let also = format!("{ALSO}:{}", s.port);
    let other_page = format!("http://{also}");

    // From the page served at another address, from another site, or with no
    // Origin: refused, nothing approved.
    for (host, origin) in [
        (also.as_str(), Some(other_page.as_str())),
        (local.as_str(), Some("http://evil.example")),
        (local.as_str(), Some(other_page.as_str())),
        (local.as_str(), None),
        (also.as_str(), Some(this_mac.as_str())),
    ] {
        let (status, body) = s.post_empty("/design/1/approve", host, origin);
        assert_eq!(status, 403, "{host} {origin:?}");
        assert!(body.contains("only on this Mac"), "{body}");
    }
    assert!(!approved());

    // A change no project has.
    let (status, body) = s.post_empty("/design/9/approve", &local, Some(&this_mac));
    assert_eq!(status, 500, "{body}");
    assert!(body.contains("no project has design change"), "{body}");

    let (status, body) = s.post_empty("/design/1/approve", &local, Some(&this_mac));
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"approved": true, "already": false, "session": "told"})
    );
    assert!(!body.contains("tok-9c1d"));
    let lines = inbox.take().unwrap().received();
    assert!(
        lines[0].contains("tok-9c1d") && lines[1].contains("Design change #1 approved: land it.")
    );

    // Again: already approved, nothing sent (the inbox took one connection and is gone).
    let (status, body) = s.post_empty("/design/1/approve", &local, Some(&this_mac));
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"already\":true"), "{body}");
    assert!(approved());
    // The page served at localhost is this Mac too.
    let localhost = format!("localhost:{}", s.port);
    let (status, body) = s.post_empty(
        "/design/1/approve",
        &localhost,
        Some(&format!("http://{localhost}")),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"already\":true"), "{body}");
    // The page's own data never carries the token.
    let (_, _, page) = s.get("/design/1");
    assert!(!page.contains("tok-9c1d"));
}

/// The one gate: a request that changes state passes only from a loopback
/// peer with a loopback Host and Origin among the daemon's own names, whatever
/// the route; a read passes from anywhere the Host check allows.
#[test]
fn a_peer_that_is_not_loopback_changes_nothing_on_any_route_that_changes_state() {
    let names: Vec<String> = vec![
        "127.0.0.1:7787".into(),
        "localhost:7787".into(),
        "[::1]:7787".into(),
        "192.0.2.7:7787".into(),
    ];
    let asker = |here: bool, host: &'static str, origin: Option<&'static str>| {
        let asker = Asker {
            here,
            host: Some(host),
            origin,
            names: &names,
        };
        // Borrow-free copy of the pieces the closure needs.
        (asker.here, asker.host, asker.origin)
    };
    let check = |method: &str, here: bool, host: &'static str, origin: Option<&'static str>| {
        let (here, host, origin) = asker(here, host, origin);
        allowed(
            method,
            &Asker {
                here,
                host,
                origin,
                names: &names,
            },
        )
    };
    let routes = ["POST", "PUT", "PATCH", "DELETE", "OPTIONS", "SOMETHING"];
    for method in routes {
        assert!(
            check(
                method,
                true,
                "127.0.0.1:7787",
                Some("http://127.0.0.1:7787")
            ),
            "{method} from this Mac"
        );
        assert!(check(
            method,
            true,
            "localhost:7787",
            Some("http://localhost:7787")
        ));
        assert!(check(method, true, "[::1]:7787", Some("http://[::1]:7787")));
        // A peer that is not loopback, with every header right: refused.
        assert!(
            !check(
                method,
                false,
                "127.0.0.1:7787",
                Some("http://127.0.0.1:7787")
            ),
            "{method} from another machine with forged loopback headers"
        );
        assert!(!check(
            method,
            false,
            "localhost:7787",
            Some("http://localhost:7787")
        ));
        assert!(!check(
            method,
            false,
            "192.0.2.7:7787",
            Some("http://192.0.2.7:7787")
        ));
        // A loopback peer on a page served at another address, or no origin.
        assert!(!check(
            method,
            true,
            "192.0.2.7:7787",
            Some("http://192.0.2.7:7787")
        ));
        assert!(!check(method, true, "127.0.0.1:7787", None));
        assert!(!check(
            method,
            true,
            "127.0.0.1:7787",
            Some("http://evil.example")
        ));
    }
    for method in ["GET", "HEAD"] {
        assert!(
            check(method, false, "192.0.2.7:7787", None),
            "{method} reads"
        );
    }
    // The routes that change state, by path: `handle` gates by method before it
    // looks at the path, so these are refused before they run.
    assert_eq!(design_approve_id("/design/3/approve"), Some(3));
}
