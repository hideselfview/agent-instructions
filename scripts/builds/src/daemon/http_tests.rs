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
    std::thread::spawn(move || serve(listener, port, serving));
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
/// `wt` in the scratch folder: a board `A` and the design system's files. Its
/// review page has two boards, `A` and one that is no file.
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
    let store = make_store(places, &["bae", "coven"]);
    store
        .write(&stamp(NOON), "design record", |db| {
            design_drawn(
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
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_daemon_draws_a_design_changes_review_page_and_its_boards_from_the_worktree_on_each_request()
{
    let s = server_with("http-design", design_store);
    let (status, kind, page) = s.get("/design/1");
    assert_eq!((status, kind.as_str()), (200, "text/html; charset=utf-8"));
    assert!(
        page.contains("\"heading\":\"The change\"") && page.contains("/design/1/board/0"),
        "{page}"
    );
    let (status, kind, board) = s.get("/design/1/board/0");
    assert_eq!((status, kind.as_str()), (200, "text/html; charset=utf-8"));
    assert!(
        board.contains("board A as drawn") && board.contains("data-theme=\"light\""),
        "{board}"
    );

    // A redraw in the worktree shows on the next request, with no restart and no new record.
    let file = s.scratch.dir.join("wt/design/mocks/A.dc.html");
    std::fs::write(&file, "<x-dc><p>board A redrawn</p></x-dc>").unwrap();
    assert!(s.get("/design/1/board/0").2.contains("board A redrawn"));

    // A board that is no file is its own problem, said where the frame is.
    let (status, _, problem) = s.get("/design/1/board/1");
    assert_eq!(status, 500);
    assert!(problem.contains("Gone.dc.html is not there"), "{problem}");
    // No such change, board or route; and the image route is gone.
    for path in [
        "/design/9",
        "/design/1/board/2",
        "/design/x",
        "/design/1/other/0",
        "/design/1/board/0/1",
        "/api/design-render/1/0",
    ] {
        assert_eq!(s.get(path).0, 404, "{path}");
    }
}

#[test]
fn a_design_path_is_a_page_or_a_board_of_a_page() {
    assert_eq!(design_route("7"), Some(DesignRoute::Page(7)));
    assert_eq!(design_route("7/board/3"), Some(DesignRoute::Board(7, 3)));
    for bad in [
        "",
        "7/",
        "a",
        "7/board",
        "7/board/-1",
        "7/boards/1",
        "7/board/1/2",
    ] {
        assert_eq!(design_route(bad), None, "{bad:?}");
    }
}

/// Project `demo` with hook runs made in a repo of its own: one before a change, one on the old base
/// after it, one on a base that has it, and a slow run.
fn hook_store(scratch: &Scratch, places: &crate::places::Places) {
    use crate::checks::Kind;
    use crate::hook_stats::{CheckRecord, DATABASE, Outcome, Run, record};
    use crate::testing::{git, repo};
    let top = scratch.dir.join("hooked");
    repo(&top, "target 1\n");
    let head = |dir: &std::path::Path| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let old = head(&top);
    std::fs::write(top.join("change.txt"), "c").unwrap();
    git(&top, &["add", "change.txt"]);
    git(&top, &["commit", "-q", "-m", "The change"]);
    std::fs::write(scratch.dir.join("change"), head(&top)).unwrap();
    let new = head(&top);
    let store = crate::lists::fixtures::make_store(places, &["demo"]);
    let run = |at: i64, base: &str, total_ms: u64, clippy_ms: u64| Run {
        at,
        repo: "demo".into(),
        worktree: top.to_string_lossy().into_owned(),
        branch: Some("main".into()),
        base: Some(base.into()),
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
    for r in [
        run(1, &old, 80_000, 70_000),
        run(2_000_000_000, &old, 90_000, 85_000),
        run(2_000_000_001, &new, 20_000, 10_000),
    ] {
        record(&store.dir.join(DATABASE), &r).unwrap();
    }
}

#[test]
fn the_daemon_serves_the_hook_runs_with_their_flag_and_splits_them_by_a_change_on_request() {
    let s = server_with("http-hooks", hook_store);
    let change = std::fs::read_to_string(s.scratch.dir.join("change")).unwrap();
    let (status, kind, body) = s.get("/api/hooks");
    assert_eq!((status, kind.as_str()), (200, "application/json"));
    let hooks: serde_json::Value = serde_json::from_str(&body).unwrap();
    // The newest run of the one check set took 20 s: under the limit, so nothing is flagged.
    assert_eq!(hooks["projects"][0]["over_limit"], serde_json::json!([]));
    let (status, kind, body) = s.get(&format!(
        "/api/hooks/impact?project=demo&change={}",
        &change[..10]
    ));
    assert_eq!((status, kind.as_str()), (200, "application/json"));
    let impact: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        (
            impact["with"]["runs"].as_u64(),
            impact["without"]["runs"].as_u64()
        ),
        (Some(1), Some(2)),
        "{impact}"
    );
    assert_eq!(impact["with"]["checks"][0]["median_crates"], 4);
    assert_eq!(impact["without"]["checks"][0]["median_ms"], 77_500);
    // A change git does not know, a project that is not one, and a request missing a part.
    let (status, _, body) = s.get("/api/hooks/impact?project=demo&change=nonesuch");
    assert!(
        status == 200 && body.contains("\"error\""),
        "{status} {body}"
    );
    let (_, _, body) = s.get(&format!(
        "/api/hooks/impact?project=other&change={}",
        &change[..10]
    ));
    assert!(body.contains("no project"), "{body}");
    assert_eq!(s.get("/api/hooks/impact?project=demo").0, 400);
}
