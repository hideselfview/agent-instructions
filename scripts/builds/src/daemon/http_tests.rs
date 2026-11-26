use super::*;
use crate::daemon::runs::{Phase, RunPage, Runs};
use crate::testing::Scratch;
use std::net::TcpListener;

#[path = "http_coverage_tests.rs"]
mod coverage;

/// A daemon's page server on a port of its own, over a scratch volume whose
/// `demo` repo has a mac app checkout. Its build-app is a shell script that
/// waits for the `gate` file, then ends with status 3.
struct Server {
    scratch: Scratch,
    port: u16,
    model: Arc<Model>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.model.runs.kill_started();
        self.model.stop_sampling();
    }
}

const WAIT_THEN_FAIL: &str = "while [ ! -e \"$0\" ]; do sleep 0.05; done; echo \"ran: $*\"; exit 3";

fn server(name: &str) -> Server {
    server_with(name, |_, _| {})
}

/// `server`, after <prepare> has made what the daemon reads at start (the
/// scratch volume's lists, say) in the scratch folder and the places.
fn server_with(name: &str, prepare: impl FnOnce(&Scratch, &mut crate::places::Places)) -> Server {
    let scratch = Scratch::new(name);
    let tool = scratch.tool(0);
    let mut places = tool.places.clone();
    prepare(&scratch, &mut places);
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.root.join("demo/app-mac-checkouts"), "app-1\n").unwrap();
    let gate = scratch.dir.join("gate");
    let program = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        WAIT_THEN_FAIL.to_string(),
        gate.to_string_lossy().into_owned(),
    ];
    let runs = Runs::start(places.clone(), program).unwrap();
    let model = Model::start(places, tool.rules.clone(), runs, tool.program.clone()).unwrap();
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
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
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
        let reply = self.get_raw(path);
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

    /// The whole reply to a GET of <path>, head and body.
    fn get_raw(&self, path: &str) -> String {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            self.port
        );
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }
}

/// A store of project `demo` holding open design change 1, whose worktree is
/// `wt` in the scratch folder, a repo with a commit holding a board `A` and
/// the design system's files, which is the change's last commit. Its review
/// page has two boards, `A` and one that is no file.
fn design_store(scratch: &Scratch, places: &crate::places::Places) {
    use crate::lists::design::ReviewSection;
    use crate::lists::fixtures::{NOON, design_drawn, make_store, stamp};
    use crate::lists::{ReviewEntry, Shown, Theme};
    let wt = scratch.dir.join("wt");
    std::fs::create_dir_all(wt.join("design/mocks")).unwrap();
    crate::testing::design_system(&wt, "1", "", "window.Bae = {};");
    std::fs::write(
        wt.join("design/mocks/A.dc.html"),
        "<x-dc><p>board A as drawn {{yes}}</p></x-dc>",
    )
    .unwrap();
    std::fs::write(
        wt.join("design/mocks/B.dc.html"),
        "<x-dc><p>board B off the page</p></x-dc>",
    )
    .unwrap();
    std::fs::write(
        wt.join("design/mocks/canvas.json"),
        r#"{"boards": {"A.dc.html": {"w": 300, "h": 200}, "B.dc.html": {"w": 30, "h": 20}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(wt.join("design/system/components/MacRow")).unwrap();
    std::fs::write(
        wt.join("design/system/components/MacRow/preview.html"),
        "<!-- @dsCard height=100 -->\n<html><head></head><body>the row preview</body></html>",
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
                &["design/mocks/B.dc.html"],
                Some(ReviewSection {
                    heading: "The change".into(),
                    boards: vec![
                        ReviewEntry {
                            shown: Shown::Board {
                                path: "design/mocks/A.dc.html".into(),
                                theme: Theme::Light,
                            },
                            caption: "A".into(),
                            show_now: true,
                            circles: vec![],
                        },
                        ReviewEntry {
                            shown: Shown::Board {
                                path: "design/mocks/Gone.dc.html".into(),
                                theme: Theme::Dark,
                            },
                            caption: "Gone".into(),
                            show_now: false,
                            circles: vec![],
                        },
                        ReviewEntry {
                            shown: Shown::Component("MacRow".into()),
                            caption: "The row".into(),
                            show_now: true,
                            circles: vec![],
                        },
                    ],
                }),
            );
            crate::lists::fixtures::design_tipped(db, id, &tip);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_board_names_the_sf_mono_files_and_the_daemon_serves_them_and_only_them() {
    let s = server_with("http-fonts", |scratch, places| {
        design_store(scratch, places);
        let fonts = scratch.dir.join("fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        for (file, ..) in boards::fonts::FACES {
            std::fs::write(fonts.join(file), "font bytes").unwrap();
        }
        std::fs::write(fonts.join("secret.txt"), "secret").unwrap();
        places.fonts = Some(fonts);
    });
    let (_, _, board) = s.get("/design/demo/1/board/0/proposed");
    let rules = boards::fonts::font_face_css(s.model.places.fonts.as_ref().unwrap()).unwrap();
    assert_eq!(rules.lines().count(), 12);
    assert!(board.contains(&rules), "{board}");
    let reply = s.get_raw("/fonts/sf-mono/SF-Mono-Regular.otf");
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    assert!(reply.contains("Content-Type: font/otf"), "{reply}");
    assert!(reply.contains("Access-Control-Allow-Origin: *"), "{reply}");
    assert!(reply.ends_with("font bytes"), "{reply}");
    for refused in ["secret.txt", "..%2Fsecret.txt", "SF-Mono-Unknown.otf"] {
        let (status, _, body) = s.get(&format!("/fonts/sf-mono/{refused}"));
        assert_eq!(status, 404, "{body}");
    }
}

#[test]
fn the_daemon_draws_a_design_changes_review_page_and_its_boards_from_the_changes_last_commit_on_each_request()
 {
    let s = server_with("http-design", |scratch, places| {
        design_store(scratch, places)
    });
    let (status, kind, page) = s.get("/design/demo/1");
    assert_eq!((status, kind.as_str()), (200, "text/html; charset=utf-8"));
    assert!(page.contains("/design/demo/1/board/0"), "{page}");
    assert!(
        page.contains("/design/demo/1/board/0/now")
            && page.contains("/design/demo/1/board/0/proposed")
    );
    let (status, kind, board) = s.get("/design/demo/1/board/0/proposed");
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
    let (_, _, board) = s.get("/design/demo/1/board/0/proposed");
    assert!(
        board.contains("board A as drawn") && !board.contains("redrawn"),
        "{board}"
    );
    // The now is origin/main, which has moved on to that other board.
    assert!(
        s.get("/design/demo/1/board/0/now")
            .2
            .contains("board A as drawn")
    );
    crate::testing::git(&wt, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let (_, _, now) = s.get("/design/demo/1/board/0/now");
    assert!(now.contains("board A redrawn"), "{now}");

    // A board that is no file is its own problem, said where the frame is.
    let (status, _, problem) = s.get("/design/demo/1/board/1/proposed");
    assert_eq!(status, 500);
    assert!(problem.contains("Gone.dc.html is not there"), "{problem}");
    // A component entry draws its preview, on each side, with the script that
    // sizes the frame and the one that answers the page's circles.
    for side in ["now", "proposed"] {
        let (status, kind, preview) = s.get(&format!("/design/demo/1/board/2/{side}"));
        assert_eq!((status, kind.as_str()), (200, "text/html; charset=utf-8"));
        assert!(
            preview.contains("the row preview")
                && preview.contains("previewWidth")
                && preview.contains("designCircles"),
            "{preview}"
        );
    }
    // A board the change changes that the page does not show opens from
    // the page's list as its proposal.
    let (status, _, other) = s.get("/design/demo/1/changed/0/proposed");
    assert_eq!(status, 200);
    assert!(other.contains("board B off the page"), "{other}");
    // No such change, board or route; and the image route is gone.
    for path in [
        "/design/demo/1/changed/1/proposed",
        "/design/demo/1/changed/0/now",
        "/design/demo/9",
        "/design/1",
        "/design/other/1",
        "/design/demo/1/board/3/now",
        "/design/demo/1/board/0",
        "/design/demo/1/deleted/0/now",
        "/design/demo/x",
        "/design/demo/1/other/0",
        "/design/demo/1/board/0/1/2",
        "/api/design-render/1/0",
    ] {
        assert_eq!(s.get(path).0, 404, "{path}");
    }
}

#[test]
fn a_design_path_is_a_page_or_a_board_of_a_page() {
    assert_eq!(design_route("demo/7"), Some(DesignRoute::Page("demo", 7)));
    // Each project counts its own ids, so a bare id names no page.
    assert_eq!(design_route("7"), None);
    assert_eq!(
        design_route("demo/7/board/3/now"),
        Some(DesignRoute::Board("demo", 7, 3, Side::Now))
    );
    assert_eq!(
        design_route("demo/7/board/3/proposed"),
        Some(DesignRoute::Board("demo", 7, 3, Side::Proposed))
    );
    assert_eq!(
        design_route("demo/7/deleted/2/now"),
        Some(DesignRoute::Deleted("demo", 7, 2))
    );
    assert_eq!(
        design_route("demo/7/changed/4/proposed"),
        Some(DesignRoute::Changed("demo", 7, 4))
    );
    for bad in [
        "",
        "demo/7/",
        "a",
        "7",
        "7/board/3/now",
        "demo",
        "demo/a",
        "demo/7/board",
        "demo/7/board/-1/now",
        "demo/7/board/3",
        "demo/7/board/3/then",
        "demo/7/deleted/2",
        "demo/7/deleted/2/proposed",
        "demo/7/changed/4/now",
        "demo/7/changed/4",
        "demo/7/boards/1",
        "demo/7/board/1/2/3",
        "demo/7/variant/1",
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
            outcome: Outcome::Ran {
                result: crate::step_failure::RunResult::Passed,
                handed: (1, 4),
                ms: Some(clippy_ms),
                started: None,
                cargo: None,
            },
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
    let s = server_with("http-hooks", |scratch, places| hook_store(scratch, places));
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
    use crate::inbox::Messaging;
    use crate::lists::design_session::keep;
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
        let (status, body) = s.post_empty("/design/demo/1/approve", host, origin);
        assert_eq!(status, 403, "{host} {origin:?}");
        assert!(body.contains("only on this Mac"), "{body}");
    }
    assert!(!approved());

    // A change no project has.
    let (status, body) = s.post_empty("/design/demo/9/approve", &local, Some(&this_mac));
    assert_eq!(status, 500, "{body}");
    assert!(body.contains("project demo has no design change"), "{body}");

    let (status, body) = s.post_empty("/design/demo/1/approve", &local, Some(&this_mac));
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"approved": true, "already": false, "session": "told"})
    );
    assert!(!body.contains("tok-9c1d"));
    let lines = inbox.take().unwrap().received();
    assert!(
        lines[0].contains("tok-9c1d")
            && lines[1].contains("Design change #1 of project demo approved: land it.")
    );

    // Again: already approved, nothing sent (the inbox took one connection and is gone).
    let (status, body) = s.post_empty("/design/demo/1/approve", &local, Some(&this_mac));
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"already\":true"), "{body}");
    assert!(approved());
    // The page served at localhost is this Mac too.
    let localhost = format!("localhost:{}", s.port);
    let (status, body) = s.post_empty(
        "/design/demo/1/approve",
        &localhost,
        Some(&format!("http://{localhost}")),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"already\":true"), "{body}");
    // The page's own data never carries the token.
    let (_, _, page) = s.get("/design/demo/1");
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
    assert_eq!(
        design_approve_target("/design/demo/3/approve"),
        Some(("demo", 3))
    );
}

#[test]
fn the_effect_control_works_only_from_the_page_at_this_mac() {
    use crate::lists::fixtures::{NOON, make_store, question, stamp};
    let s = server_with("http-effect", |_, places| {
        make_store(places, &["bae"])
            .write(&stamp(NOON), "question ask", |db| Ok(question(db)))
            .unwrap();
    });
    let effect = || {
        let store = crate::lists::store::Store::named(&s.model.places, "demo").unwrap();
        store
            .load()
            .unwrap()
            .question(crate::lists::Id(1))
            .unwrap()
            .effect
    };
    let local = format!("127.0.0.1:{}", s.port);
    let this_mac = format!("http://{local}");
    let also = format!("{ALSO}:{}", s.port);
    let body = r#"{"project": "demo", "id": 1, "effect": "bug"}"#;
    let (status, reply) = s.post_to(
        "/api/question-effect",
        &also,
        Some(&format!("http://{also}")),
        body,
    );
    assert_eq!(status, 403, "{reply}");
    assert_eq!(effect(), crate::lists::Effect::Cleanup);
    let (status, reply) = s.post_to(
        "/api/question-effect",
        &local,
        Some(&this_mac),
        r#"{"project": "demo", "id": 1, "effect": "worse"}"#,
    );
    assert_eq!(status, 500, "{reply}");
    assert!(reply.contains("the effect is data-loss, bug,"), "{reply}");
    let (status, reply) = s.post_to("/api/question-effect", &local, Some(&this_mac), body);
    assert_eq!((status, reply.as_str()), (200, "{}"));
    assert_eq!(effect(), crate::lists::Effect::Bug);
}

#[test]
fn the_start_button_works_only_from_the_page_at_this_mac_and_records_the_dashboard_as_its_author() {
    use crate::lists::fixtures::{NOON, make_store, stamp};
    let s = server_with("http-start", |_, places| {
        make_store(places, &["bae"])
            .write(&stamp(NOON), "item add", |db| {
                let id = crate::lists::fixtures::item(db, vec![]);
                crate::lists::item::move_to_backlog(db, &[(id, "later".to_string())], &stamp(NOON))
            })
            .unwrap();
    });
    let in_backlog = || {
        let store = crate::lists::store::Store::named(&s.model.places, "demo").unwrap();
        store
            .load()
            .unwrap()
            .item(crate::lists::Id(1))
            .unwrap()
            .backlog
            .is_some()
    };
    let local = format!("127.0.0.1:{}", s.port);
    let this_mac = format!("http://{local}");
    let also = format!("{ALSO}:{}", s.port);
    let body = r#"{"project": "demo", "id": 1}"#;
    let (status, reply) = s.post_to(
        "/api/item-start",
        &also,
        Some(&format!("http://{also}")),
        body,
    );
    assert_eq!(status, 403, "{reply}");
    assert!(in_backlog());
    let (status, reply) = s.post_to("/api/item-start", &local, Some(&this_mac), body);
    assert_eq!((status, reply.as_str()), (200, "{}"));
    assert!(!in_backlog());
    let store = crate::lists::store::Store::named(&s.model.places, "demo").unwrap();
    let db = crate::db::open_reader(&store.database()).unwrap().unwrap();
    let logged = crate::lists::store::read_log(&db).unwrap().pop().unwrap();
    assert_eq!(
        (logged.2.as_str(), logged.3.as_str()),
        ("person (dashboard)", "item start 1")
    );
    let item = store
        .load()
        .unwrap()
        .item(crate::lists::Id(1))
        .unwrap()
        .clone();
    assert_eq!(
        item.notes
            .last()
            .map(|note| (note.by.as_str(), note.text.as_str())),
        Some(("person (dashboard)", "started: started on the dashboard"))
    );
}

#[test]
fn a_designs_path_is_the_page_or_a_board_or_component_at_a_full_commit() {
    let commit = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(designs_route("bae"), Some(DesignsRoute::Page("bae")));
    assert_eq!(
        designs_route(&format!("bae/{commit}/board/mac.welcome")),
        Some(DesignsRoute::Board("bae", commit, "mac.welcome"))
    );
    assert_eq!(
        designs_route(&format!("bae/{commit}/component/MacRow")),
        Some(DesignsRoute::Component("bae", commit, "MacRow"))
    );
    assert_eq!(
        designs_route(&format!("bae/{commit}/colors/3")),
        Some(DesignsRoute::Colors("bae", commit, 3))
    );
    assert_eq!(
        designs_route("bae/component/MacCheckbox"),
        Some(DesignsRoute::ComponentShots("bae", "MacCheckbox"))
    );
    assert_eq!(
        designs_route(&format!(
            "bae/{commit}/shot/mac/mac.component.checkbox.on/board.png"
        )),
        Some(DesignsRoute::Picture(
            "bae",
            commit,
            "mac",
            "mac.component.checkbox.on",
            PictureSide::Board
        ))
    );
    for bad in [
        "".to_string(),
        "bae/".to_string(),
        "bae/component/".to_string(),
        format!("bae/{commit}/shot/mac/mac.a.b/front.png"),
        format!("bae/{commit}/shot/mac/mac.a.b/board"),
        "bae/HEAD/board/A".to_string(),
        format!("bae/{commit}/board"),
        format!("bae/{commit}/colors/one"),
        format!("bae/{commit}/boards/A"),
        format!("bae/{commit}/board/A/now"),
    ] {
        assert_eq!(designs_route(&bad), None, "{bad:?}");
    }
}

#[test]
fn a_shots_path_names_a_project_commit_component_and_what_it_asks() {
    let commit = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        shots_target(&format!("/designs/bae/{commit}/shots/MacCheckbox/events")),
        Some(("bae", commit, "MacCheckbox", ShotsAsk::Events))
    );
    assert_eq!(
        shots_target(&format!("/designs/bae/{commit}/shots/MacCheckbox/capture")),
        Some(("bae", commit, "MacCheckbox", ShotsAsk::Capture))
    );
    for bad in [
        format!("/designs/bae/{commit}/shots/MacCheckbox"),
        format!("/designs/bae/{commit}/shots/MacCheckbox/run"),
        "/designs/bae/main/shots/MacCheckbox/events".to_string(),
        format!("/design/bae/{commit}/shots/MacCheckbox/events"),
    ] {
        assert_eq!(shots_target(&bad), None, "{bad:?}");
    }
}

#[test]
fn a_publish_path_names_one_project() {
    assert_eq!(publish_target("/designs/bae/publish"), Some("bae"));
    for bad in [
        "/designs//publish",
        "/designs/bae/x/publish",
        "/designs/bae",
        "/design/bae/publish",
    ] {
        assert_eq!(publish_target(bad), None, "{bad:?}");
    }
}

/// Reads the server-sent events of <reader> until one named <name> comes, and
/// returns its data.
fn next_event(reader: &mut BufReader<TcpStream>, name: &str) -> String {
    let mut event = String::new();
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0, "the stream ended");
        if let Some(found) = line.strip_prefix("event: ") {
            event = found.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data: ")
            && event == name
        {
            return data.trim().to_string();
        }
    }
}

#[test]
fn the_dashboard_stream_sends_lists_and_hooks_when_the_volume_is_gone() {
    let s = server_with("http-stream-without-volume", |scratch, places| {
        hook_store(scratch, places)
    });
    std::fs::rename(&s.model.places.root, s.scratch.dir.join("other-volume")).unwrap();

    let host = format!("127.0.0.1:{}", s.port);
    let mut stream = TcpStream::connect(&host).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "GET /events HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
    let mut events = BufReader::new(stream);
    let error = next_event(&mut events, "failure");
    assert!(error.contains("reading the free space"), "{error}");
    let lists: serde_json::Value = serde_json::from_str(&next_event(&mut events, "lists")).unwrap();
    assert_eq!(lists["projects"][0]["project"], "demo");
    let hooks: serde_json::Value = serde_json::from_str(&next_event(&mut events, "hooks")).unwrap();
    assert_eq!(hooks["projects"][0]["repos"][0]["runs"], 2);
}

#[test]
fn the_publish_events_follow_the_store_and_a_project_with_no_design_files_has_no_publish() {
    use crate::lists::fixtures::{NOON, make_store, stamp};
    let mut inbox = None;
    let s = server_with("http-publish", |scratch, places| {
        make_store(places, &["bae"]);
        let socket = scratch.dir.join("p.sock");
        inbox = Some(crate::testing::FakeInbox::listen(&socket));
        let messaging = crate::inbox::Messaging::new(socket.to_str().unwrap(), None).unwrap();
        crate::lists::store::Store::named(places, "demo")
            .unwrap()
            .write(&stamp(NOON), "design listen", |db| {
                crate::lists::publish::listen(db, &messaging, &stamp(NOON))
            })
            .unwrap();
    });
    let origin = format!("http://127.0.0.1:{}", s.port);
    let (status, body) = s.post_to(
        "/designs/demo/publish",
        &format!("127.0.0.1:{}", s.port),
        Some(&origin),
        r#"{"commit":"origin/main"}"#,
    );
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("project demo has no design files"), "{body}");
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    write!(
        stream,
        "GET /designs/demo/publish/events HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        s.port
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let first: serde_json::Value =
        serde_json::from_str(&next_event(&mut reader, "publish")).unwrap();
    assert_eq!(first, serde_json::json!({ "latest": null }));
    let store = crate::lists::store::Store::named(&s.model.places, "demo").unwrap();
    crate::lists::publish::ask_and_tell(
        &store,
        &s.model.places.sessions(),
        &"a".repeat(40),
        &stamp(NOON),
    )
    .unwrap();
    let next: serde_json::Value =
        serde_json::from_str(&next_event(&mut reader, "publish")).unwrap();
    assert_eq!(next["latest"]["id"], serde_json::json!(1));
    assert_eq!(inbox.unwrap().received().len(), 1);
}

/// Project `demo`, whose repo `bae` keeps its boards in design/mocks, with
/// bae's main checkout and its worktree `side-1`, whose files hold an edit
/// of board `A` that no commit holds and a scratch board
/// (`scratch_view::tests::checkouts`).
fn scratch_store(scratch: &Scratch, places: &crate::places::Places) {
    use crate::lists::fixtures::{NOON, make_store, stamp};
    use crate::lists::project::{GlobKind, set_design_globs};
    crate::daemon::scratch_view::tests::checkouts(scratch);
    let places_globs = std::collections::BTreeMap::from([(
        "bae".to_string(),
        vec!["design/mocks/*.dc.html".to_string()],
    )]);
    make_store(places, &["bae"])
        .write(&stamp(NOON), "lists design-globs demo", |db| {
            set_design_globs(db, GlobKind::Place, &places_globs)
        })
        .unwrap();
}

#[test]
fn the_scratch_view_draws_a_worktrees_files_and_redraws_after_each_change_only_on_this_mac() {
    let s = server_with("http-scratch", |scratch, places| {
        scratch_store(scratch, places)
    });
    let (status, kind, page) = s.get("/design/demo/scratch/side-1/A");
    assert_eq!((status, kind.as_str()), (200, "text/html; charset=utf-8"));
    assert!(
        page.contains(r#""base":"/design/demo/scratch/side-1/A""#),
        "{page}"
    );
    let (status, _, drawn) = s.get("/design/demo/scratch/side-1/A/light/worktree");
    assert_eq!(status, 200, "{drawn}");
    assert!(
        drawn.contains("board A edited") && drawn.contains(r#"data-theme="light""#),
        "{drawn}"
    );
    let (_, _, now) = s.get("/design/demo/scratch/side-1/A/dark/now");
    assert!(
        now.contains("board A") && !now.contains("edited") && now.contains(r#"data-theme="dark""#),
        "{now}"
    );
    assert_eq!(s.get("/design/other/scratch/side-1/A").0, 404);
    let (status, _, problem) = s.get("/design/demo/scratch/side-9/A");
    assert_eq!(status, 500);
    assert!(problem.contains("has no worktree side-9"), "{problem}");

    // At the address the dashboard also listens on, which another machine
    // reaches, the scratch view answers nothing.
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    write!(
        stream,
        "GET /design/demo/scratch/side-1/A/dark/worktree HTTP/1.1\r\nHost: {ALSO}:{}\r\n\r\n",
        s.port
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
    assert!(!reply.contains("board A"), "{reply}");

    // The events: a drawing on connect, and another after a change to the
    // worktree's design files.
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    stream.set_read_timeout(Some(SCRATCH_HANG_GUARD)).unwrap();
    write!(
        stream,
        "GET /design/demo/scratch/side-1/A/events HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        s.port
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let first: serde_json::Value =
        serde_json::from_str(&next_event(&mut reader, "drawing")).unwrap();
    assert_eq!(
        first,
        serde_json::json!({ "now": { "w": 300, "h": 200 }, "worktree": { "w": 500, "h": 100 } })
    );
    let canvas = crate::checkout::worktrees_folder(&s.scratch.repo_path("bae"))
        .unwrap()
        .join("side-1/design/mocks/canvas.json");
    std::fs::write(
        &canvas,
        r#"{"boards": {"A.dc.html": {"w": 640, "h": 480}}}"#,
    )
    .unwrap();
    loop {
        let next: serde_json::Value =
            serde_json::from_str(&next_event(&mut reader, "drawing")).unwrap();
        if next["worktree"] == serde_json::json!({ "w": 640, "h": 480 }) {
            break;
        }
    }
}

/// The limit on a read of the scratch view's events: a guard against a stuck
/// stream, not the pass condition.
const SCRATCH_HANG_GUARD: Duration = Duration::from_secs(300);

/// The data of the page at <path>, asked for at <host>.
fn page_data_at(s: &Server, path: &str, host: &str) -> serde_json::Value {
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}:{}\r\n\r\n",
        s.port
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    let start = reply
        .find("<script type=\"application/json\" id=\"data\">")
        .expect("the data")
        + "<script type=\"application/json\" id=\"data\">".len();
    let end = start + reply[start..].find("</script>").unwrap();
    serde_json::from_str(&reply[start..end]).unwrap()
}

#[test]
fn the_designs_page_distinguishes_missing_designs_from_an_unreadable_store_folder() {
    let readable = server_with("http-designs-no-project-designs", |_, places| {
        crate::lists::fixtures::make_store(places, &["bae"]);
    });
    for project in ["missing", "demo"] {
        let (status, _, body) = readable.get(&format!("/designs/{project}"));
        assert_eq!(status, 404, "{body}");
    }

    let unreadable = server_with("http-designs-unreadable-store-folder", |_, places| {
        std::fs::write(&places.lists, "The lists path is a file.").unwrap();
    });
    let (status, _, body) = unreadable.get("/designs/demo");
    assert_eq!(status, 500, "{body}");
    assert!(
        body.contains(&format!(
            "listing {}",
            unreadable.model.places.lists.display()
        )),
        "{body}"
    );
}

#[test]
fn the_designs_page_reports_a_store_in_a_folder_with_an_invalid_project_name() {
    let server = server_with("http-designs-invalid-project-folder", |_, places| {
        let store = crate::lists::fixtures::make_store(places, &["bae"]);
        std::fs::rename(store.dir, places.lists.join(".invalid")).unwrap();
    });
    let (status, _, body) = server.get("/designs/demo");
    assert_eq!(status, 500, "{body}");
    assert!(body.contains("'.invalid' is not a project name"), "{body}");
}

#[test]
fn the_designs_page_lists_the_scratch_boards_only_on_this_mac() {
    let s = server_with("http-scratch-list", |scratch, places| {
        scratch_store(scratch, places)
    });
    assert_eq!(
        page_data_at(&s, "/designs/demo", "127.0.0.1")["scratch"],
        serde_json::json!({ "boards": [{
            "worktree": "side-1",
            "board": "scratch.rows",
            "link": "/design/demo/scratch/side-1/scratch.rows",
        }] })
    );
    // Another machine sees no scratch board, and no name of one.
    assert_eq!(
        page_data_at(&s, "/designs/demo", ALSO)["scratch"],
        serde_json::Value::Null
    );
    // The scratch view sizes a scratch board's frame to what it draws.
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    stream.set_read_timeout(Some(SCRATCH_HANG_GUARD)).unwrap();
    write!(
        stream,
        "GET /design/demo/scratch/side-1/scratch.rows/events HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        s.port
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let first: serde_json::Value =
        serde_json::from_str(&next_event(&mut reader, "drawing")).unwrap();
    assert_eq!(
        first,
        serde_json::json!({ "now": { "absent": true }, "worktree": { "fit": true } })
    );
    let (status, _, drawn) = s.get("/design/demo/scratch/side-1/scratch.rows/dark/worktree");
    assert_eq!(status, 200, "{drawn}");
    assert!(
        drawn.contains("rows two") && drawn.contains("designSize"),
        "{drawn}"
    );
}

#[test]
fn a_review_page_links_the_scratch_boards_of_its_changes_worktree_only_on_this_mac() {
    let s = server_with("http-scratch-review", |scratch, places| {
        use crate::lists::fixtures::{NOON, design_drawn, stamp};
        scratch_store(scratch, places);
        let main = scratch.repo_path("bae");
        // A scratch board of another worktree, which the page leaves out.
        std::fs::write(
            main.join("design/mocks/scratch.other.dc.html"),
            "<x-dc></x-dc>",
        )
        .unwrap();
        let side = crate::checkout::worktrees_folder(&main)
            .unwrap()
            .join("side-1");
        crate::lists::store::Store::named(places, "demo")
            .unwrap()
            .write(&stamp(NOON), "design record", |db| {
                design_drawn(db, &side.display().to_string(), &[], None);
                Ok(())
            })
            .unwrap();
    });
    let scratch = &page_data_at(&s, "/design/demo/1", "127.0.0.1")["scratch"];
    let boards = scratch["boards"].as_array().unwrap();
    assert_eq!(boards.len(), 1, "{scratch}");
    assert_eq!(
        boards[0]["link"],
        serde_json::json!("/design/demo/scratch/side-1/scratch.rows")
    );
    assert_eq!(
        page_data_at(&s, "/design/demo/1", ALSO)["scratch"],
        serde_json::Value::Null
    );
}
