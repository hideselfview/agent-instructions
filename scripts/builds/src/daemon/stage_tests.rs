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
        "pages": [{ "id": "p1", "name": "Page one", "boards": [{ "name": "A", "title": null, "x": 0.0, "y": 0.0, "w": 10, "h": 10 }], "headings": [{ "name": "Row", "y": 0.0, "boards": ["A"] }] }],
        "system": { "styles": [{ "name": "body", "css": "font-size: 13px", "words": "Words" }] },
        "colors": {
            "themes": [{ "id": "dark", "name": "Dark" }],
            "switches": [],
            "comparison": null,
            "groups": [{ "name": "Text", "about": "Words.", "colors": ["text"] }],
        },
        "components": {
            "groups": [{
                "name": "Controls",
                "about": "Buttons.",
                "cards": [{ "name": "Button", "height": 40 }],
                "subgroups": [{
                    "name": "Rows",
                    "about": "Lists.",
                    "cards": [{ "name": "Row", "error": "no preview" }],
                }],
            }],
            "problems": [],
        },
        "scratch": { "boards": [{ "worktree": "side-1", "board": "scratch.a", "link": "/x" }] },
    })
}

/// The hooks of one project that holds a repo with runs, app builds and waits.
fn hooks() -> Value {
    let run = json!({
        "at": 1, "repo": "demo", "worktree": "side-1", "branch": null, "base": null,
        "plan_ms": 5, "total_ms": 50, "files": 1, "added": 2, "removed": 1,
        "kinds": { "rs": 1 }, "crates": 1,
        "checks": [{
            "name": "fmt", "kind": "check", "outcome": "passed", "ms": 10, "crates": 1,
            "place": null,
        }],
    });
    json!({ "projects": [{
        "project": "demo",
        "repos": [{
            "repo": "demo", "runs": 1, "median_ms": 50, "slowest_ms": 50,
            "checks": [{
                "name": "fmt", "kind": "check", "ran": 1, "skipped": 0, "median_ms": 10,
                "slowest_ms": 10, "trend": { "recent_ms": 9, "earlier_ms": 10 },
            }],
            "trend_runs": 10, "limit_ms": 45_000, "over_count": 0,
            "over": [], "slowest": [run.clone()], "recent": [run],
        }],
        "app_builds": { "window_seconds": 100, "repos": [{
            "repo": "demo",
            "lanes": [{
                "lane": "mac", "runs": 1, "median_wait_ms": 3, "waits_ms": [3],
                "steps": [{
                    "name": "build", "kind": "step", "ran": 1, "median_ms": 4,
                    "slowest_ms": 4, "total_ms": 4,
                }],
            }],
        }] },
        "waits": { "rows": [{
            "kind": "cargo", "day": "2026-11-26", "band": "under 1 s", "count": 1,
            "median_ms": 5, "total_ms": 5,
        }] },
    }] })
}

fn answers() -> Value {
    json!({
        "now": 1_700_000_100,
        "state": state(),
        "lists": { "projects": [] },
        "hooks": hooks(),
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
    let listener = bind().unwrap();
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
fn the_staged_page_is_the_daemons_page_with_its_clock_at_the_fixtures_time() {
    let scratch = Scratch::new("stage-page");
    let port = staged(&fixture(&scratch, answers()));
    let (status, body) = answer(port, "GET", "/");
    assert_eq!(status, 200);
    assert!(body.contains("<title>"), "{body}");
    let (before, from_clock) = body
        .split_once("<script>\n// A staged page's clock")
        .unwrap();
    let (clock, after) = from_clock.split_once("</script>\n").unwrap();
    assert_eq!(format!("{before}{after}"), **crate::daemon::http::PAGE);
    assert!(clock.contains("const now = 1700000100 * 1000;"), "{clock}");
}

#[test]
fn the_staged_event_stream_sends_the_fixtures_answers_and_stays_open() {
    let scratch = Scratch::new("stage-events");
    let port = staged(&fixture(&scratch, answers()));
    let mut stream = request(port, "GET", "/events");
    let mut text = String::new();
    let mut buffer = [0u8; 4096];
    // Reads until all three events came. A stream that ended first reads 0
    // bytes, and a stream that sends nothing fails on the 10 s timeout.
    while !["event: state\n", "event: lists\n", "event: hooks\n"]
        .iter()
        .all(|event| text.contains(event))
    {
        match stream.read(&mut buffer).unwrap() {
            0 => panic!("the stream ended after: {text}"),
            n => text.push_str(&String::from_utf8_lossy(&buffer[..n])),
        }
    }
    // Nothing more comes, and the stream stays open.
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let kind = stream.read(&mut buffer).unwrap_err().kind();
    assert!(
        matches!(
            kind,
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
        "{kind:?}"
    );
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
    assert!(body.contains("const now = 1700000100 * 1000;"), "{body}");
    let (status, frame) = answer(port, "GET", &format!("/designs/demo/{COMMIT}/board/A"));
    assert_eq!(status, 200);
    assert!(frame.contains("<body></body>"), "{frame}");
    assert_eq!(answer(port, "GET", "/designs/other").0, 404);
}

#[test]
fn the_stage_gives_the_address_of_the_page_the_fixture_answers() {
    let scratch = Scratch::new("stage-address");
    let load = |stage| Fixture::load(&fixture(&scratch, stage), "web.demo.page").unwrap();
    assert_eq!(
        address(7000, &load(answers())),
        "http://127.0.0.1:7000/designs/demo"
    );
    let mut dashboard = answers();
    dashboard.as_object_mut().unwrap().remove("designs");
    assert_eq!(address(7000, &load(dashboard)), "http://127.0.0.1:7000/");
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
    let mut timeless = answers();
    timeless.as_object_mut().unwrap().remove("now");
    assert!(Fixture::load(&fixture(&scratch, timeless), "web.demo.page").is_err());
    let mut extra = answers();
    extra["views"] = json!([]);
    assert!(Fixture::load(&fixture(&scratch, extra), "web.demo.page").is_err());
}

#[test]
fn a_fixture_with_the_wrong_shape_inside_a_section_fails_to_load() {
    let scratch = Scratch::new("stage-sections");
    let wrong: [(&str, Value); 17] = [
        ("/hooks/projects/0/repos/0/runs", json!("one")),
        ("/hooks/projects/0/repos/0/checks/0/ran", json!(-1)),
        (
            "/hooks/projects/0/repos/0/slowest/0/checks/0/ms",
            json!("ten"),
        ),
        ("/hooks/projects/0/repos/0/limit_ms", json!("slow")),
        (
            "/hooks/projects/0/app_builds/repos/0/lanes/0/runs",
            json!("one"),
        ),
        (
            "/hooks/projects/0/app_builds/repos/0/lanes/0/steps/0/ran",
            json!(null),
        ),
        ("/hooks/projects/0/waits/rows/0/count", json!("one")),
        ("/hooks/projects/0/waits/rows/0/band", json!(3)),
        ("/designs/pages/0/boards/0/w", json!("wide")),
        ("/designs/pages/0/headings/0/boards", json!("A")),
        ("/designs/system/styles/0/css", json!(1)),
        ("/designs/colors/groups/0/colors", json!("text")),
        ("/designs/components/groups/0/cards/0/height", json!("tall")),
        ("/designs/components/problems", json!("none")),
        ("/designs/publish", json!({ "canvas": "yes" })),
        ("/designs/scratch/boards/0/link", json!(1)),
        ("/designs/colors/themes/0/id", json!(1)),
    ];
    for (place, value) in wrong {
        let mut stage = answers();
        *stage.pointer_mut(place).expect(place) = value;
        let loaded = Fixture::load(&fixture(&scratch, stage), "web.demo.page");
        assert!(loaded.is_err(), "a wrong {place} loads");
    }
    // The right shapes of the same sections load.
    Fixture::load(&fixture(&scratch, answers()), "web.demo.page").unwrap();
    for (place, value) in [
        ("/designs/publish", json!({ "error": "no canvas" })),
        ("/designs/colors", json!({ "error": "unreadable" })),
        ("/designs/scratch", json!({ "error": "no worktrees" })),
        (
            "/hooks/projects/0/app_builds",
            json!({ "error": "no database" }),
        ),
        ("/hooks/projects/0/waits", json!({ "error": "no database" })),
    ] {
        let mut stage = answers();
        *stage.pointer_mut(place).expect(place) = value;
        Fixture::load(&fixture(&scratch, stage), "web.demo.page")
            .unwrap_or_else(|error| panic!("a right {place} fails: {}", error.text()));
    }
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
