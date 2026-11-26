use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::testing::Scratch;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// The page state the daemon sends of an empty volume.
fn state() -> Value {
    json!({
        "version": 1,
        "generated_at": 1_700_000_000,
        "root": "/v",
        "volume": { "free_kib": 10, "floor_kib": 5, "hard_floor_kib": 2, "size_kib": 3 },
        "load": null,
        "health": null,
        "health_error": null,
        "hosts": [],
        "compile_cache": [],
        "compile_cache_error": null,
        "repos": [],
    })
}

fn designs() -> Value {
    json!({
        "project": "demo",
        "publish": { "canvas": false },
        "commit": COMMIT,
        "base": format!("/designs/demo/{COMMIT}"),
        "pages": [{ "id": "p1", "name": "Page one", "boards": [], "headings": [] }],
        "system": {},
        "colors": {},
        "components": {},
        "scratch": null,
    })
}

fn answers() -> Value {
    json!({
        "state": state(),
        "lists": { "projects": [] },
        "hooks": { "projects": [] },
        "designs": designs(),
    })
}

/// Writes fixture `web.demo.page` with <stage> as its stage part, and gives the
/// folder.
fn fixture(scratch: &Scratch, stage: Value) -> PathBuf {
    let dir = scratch.dir.join("fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({ "capture": { "ready": "#x", "steps": [] }, "stage": stage });
    std::fs::write(dir.join("web.demo.page.json"), file.to_string()).unwrap();
    dir
}

/// A staged page of fixture `web.demo.page` of <folder> on a port of its own.
fn staged(folder: &Path) -> u16 {
    let fixture = Fixture::load(folder, "web.demo.page").unwrap();
    let (listener, _) = bind().unwrap();
    let names: Arc<[String]> = names_of(&listener).unwrap().into();
    let port = listener.local_addr().unwrap().port();
    let pages: Arc<dyn Pages> = Arc::new(fixture);
    std::thread::spawn(move || serve_staged(listener, names, pages));
    port
}

fn request(port: u16, method: &str, path: &str) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\n\r\n"
    )
    .unwrap();
    stream
}

/// The status and body of <method> <path>.
fn answer(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut reply = String::new();
    request(port, method, path)
        .read_to_string(&mut reply)
        .unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").unwrap();
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        body.to_string(),
    )
}

#[test]
fn the_staged_page_is_the_page_the_daemon_serves() {
    let scratch = Scratch::new("stage-page");
    let port = staged(&fixture(&scratch, answers()));
    let (status, body) = answer(port, "GET", "/");
    assert_eq!(status, 200);
    assert!(body.contains("<title>"), "{body}");
    assert_eq!(body, **crate::daemon::http::PAGE);
}

#[test]
fn the_staged_event_stream_sends_the_fixtures_answers_and_stays_open() {
    let scratch = Scratch::new("stage-events");
    let port = staged(&fixture(&scratch, answers()));
    let mut stream = request(port, "GET", "/events");
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut text = String::new();
    let mut buffer = [0u8; 4096];
    // Reads until the read times out: a stream that ended would read 0 bytes
    // first.
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => panic!("the stream ended after: {text}"),
            Ok(n) => text.push_str(&String::from_utf8_lossy(&buffer[..n])),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => panic!("{error}"),
        }
    }
    for event in ["event: state\n", "event: lists\n", "event: hooks\n"] {
        assert!(text.contains(event), "no {event:?} in {text}");
    }
    assert!(text.contains("\"generated_at\":1700000000"), "{text}");
}

#[test]
fn the_staged_designs_page_fills_the_fixtures_data_and_its_frames_are_empty() {
    let scratch = Scratch::new("stage-designs");
    let port = staged(&fixture(&scratch, answers()));
    let (status, body) = answer(port, "GET", "/designs/demo");
    assert_eq!(status, 200);
    assert!(body.contains("Page one"), "{body}");
    assert!(body.contains(COMMIT), "{body}");
    let (status, frame) = answer(port, "GET", &format!("/designs/demo/{COMMIT}/board/A"));
    assert_eq!(status, 200);
    assert!(frame.contains("<body></body>"), "{frame}");
    assert_eq!(answer(port, "GET", "/designs/other").0, 404);
}

#[test]
fn a_staged_page_answers_no_post() {
    let scratch = Scratch::new("stage-post");
    let port = staged(&fixture(&scratch, answers()));
    assert_eq!(answer(port, "POST", "/api/queue").0, 405);
    assert_eq!(answer(port, "POST", "/designs/demo/publish").0, 405);
}

#[test]
fn a_fixture_of_the_wrong_shape_fails_to_load() {
    let scratch = Scratch::new("stage-wrong");
    let mut wrong = answers();
    wrong["state"]["volume"] = json!({ "free_kib": "ten" });
    let error = Fixture::load(&fixture(&scratch, wrong), "web.demo.page")
        .err()
        .expect("a state of the wrong shape fails")
        .text();
    assert!(error.contains("web.demo.page.json"), "{error}");
    for (answer, value) in [
        ("lists", json!({ "projects": "none" })),
        ("hooks", json!({ "rows": [] })),
        ("designs", json!({ "project": "demo" })),
    ] {
        let mut wrong = answers();
        wrong[answer] = value;
        assert!(
            Fixture::load(&fixture(&scratch, wrong), "web.demo.page").is_err(),
            "{answer} of the wrong shape loads"
        );
    }
    let mut extra = answers();
    extra["views"] = json!([]);
    assert!(Fixture::load(&fixture(&scratch, extra), "web.demo.page").is_err());
}

#[test]
fn the_stage_reads_its_folder_and_shot_in_either_order() {
    let words = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    let expected = (PathBuf::from("shots"), "web.demo.page".to_string());
    assert_eq!(
        stage_args(&words(&["--fixtures", "shots", "web.demo.page"])).unwrap(),
        expected
    );
    assert_eq!(
        stage_args(&words(&["web.demo.page", "--fixtures", "shots"])).unwrap(),
        expected
    );
    assert!(stage_args(&words(&["web.demo.page"])).is_err());
    assert!(stage_args(&words(&["--fixtures", "shots"])).is_err());
}
