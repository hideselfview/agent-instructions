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
    let scratch = Scratch::new(name);
    let tool = scratch.tool(0);
    let places = tool.places.clone();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.root.join("demo/app-checkouts"), "side-1\n").unwrap();
    let root = std::fs::canonicalize(&places.root).unwrap();
    let tracker = Tracker::start(&root, &places.daemon_dir()).unwrap();
    let gate = scratch.dir.join("gate");
    let program = vec!["/bin/sh".to_string(), "-c".to_string(), WAIT_THEN_FAIL.to_string(), gate.to_string_lossy().into_owned()];
    let runs = Runs::start(places.clone(), program).unwrap();
    let model = Model::start(places, tool.rules.clone(), tracker, runs);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = model.clone();
    std::thread::spawn(move || serve(listener, port, serving));
    Server { scratch, port, model }
}

impl Server {
    /// The status code and body of a POST to /api/build-app.
    fn post(&self, host: &str, origin: Option<&str>, body: &str) -> (u16, String) {
        let origin = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
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
        self.model.runs.pages("demo", &|_| Vec::new()).into_iter().next()
    }
}

const MAC: &str = r#"{"repo":"demo","lane":"mac","ref":"my-branch"}"#;

#[test]
fn a_cross_site_post_starts_no_build() {
    let s = server("http-cross-site");
    let host = format!("127.0.0.1:{}", s.port);
    assert_eq!(s.post(&host, Some("https://evil.example"), MAC).0, 403, "another site's origin");
    assert_eq!(s.post(&host, Some("null"), MAC).0, 403, "a sandboxed page's origin");
    assert_eq!(s.post(&host, Some(&format!("http://127.0.0.1:{}", s.port + 1)), MAC).0, 403, "another local port");
    assert_eq!(s.post(&host, None, MAC).0, 403, "no origin at all");
    let origin = format!("http://127.0.0.1:{}", s.port);
    assert_eq!(s.post("evil.example", Some(&origin), MAC).0, 403, "a name that resolves here");
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
        assert!(std::time::Instant::now() < deadline, "the run ended in time");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(failed.exit_code, Some(3));
    assert!(failed.output.contains("ran: build-app demo mac my-branch"), "{}", failed.output);
}
