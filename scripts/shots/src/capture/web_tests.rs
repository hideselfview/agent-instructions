use super::*;
use crate::declared::Marks;
use crate::design::Frame;
use crate::render::fixture;
use crate::shots::{Picture, Shot};
use crate::verify::{self, Drawn};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

const SHOT: &str = "web.page.main";

fn name() -> Name {
    Name::parse(SHOT, &Apps::default()).unwrap()
}

fn command(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// A stage that prints <url> and exits when its input closes.
fn staging(url: &str) -> Vec<String> {
    command(&["sh", "-c", "echo \"$0\"; cat >/dev/null", url])
}

#[tokio::test]
async fn a_stage_prints_its_url_and_exits_when_its_input_closes() {
    let stage = Stage::start(
        Path::new("."),
        &command(&[
            "sh",
            "-c",
            "echo \"http://127.0.0.1:1/$0\"; cat >/dev/null",
            "{shot}",
        ]),
        &name(),
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert_eq!(stage.url, format!("http://127.0.0.1:1/{SHOT}"));
    stage.end().await.unwrap();
}

#[tokio::test]
async fn a_stage_that_prints_no_url_fails_the_shot() {
    let error = Stage::start(
        Path::new("."),
        &command(&["sh", "-c", "exit 3"]),
        &name(),
        Duration::from_secs(20),
    )
    .await
    .err()
    .unwrap();
    assert!(error.contains("printed no URL and exited"), "{error}");
    let error = Stage::start(
        Path::new("."),
        &command(&["echo", "listening"]),
        &name(),
        Duration::from_secs(20),
    )
    .await
    .err()
    .unwrap();
    assert!(error.contains("not a URL on 127.0.0.1"), "{error}");
}

#[tokio::test]
async fn a_stage_that_never_prints_fails_when_the_deadline_passes() {
    let error = Stage::start(
        Path::new("."),
        &command(&["sh", "-c", "cat >/dev/null"]),
        &name(),
        Duration::from_millis(300),
    )
    .await
    .err()
    .unwrap();
    assert!(error.contains("printed no URL within"), "{error}");
}

#[tokio::test]
async fn a_stage_that_exits_with_an_error_fails_the_shot() {
    let stage = Stage::start(
        Path::new("."),
        &command(&[
            "sh",
            "-c",
            "echo http://127.0.0.1:1/; cat >/dev/null; exit 4",
        ]),
        &name(),
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    let error = stage.end().await.unwrap_err();
    assert!(error.contains("exited with an error"), "{error}");
}

/// The page the stage serves: a button that sets the status line, and a
/// ready marker that the page adds once a fetch answers, while an event
/// stream stays open.
const PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
body{margin:0;background:#123;color:#fff;font:16px Helvetica}\
#box{position:absolute;left:20px;top:20px;width:200px;height:100px}\
button{position:absolute;left:0;top:0;width:120px;height:32px;font:16px Helvetica}\
p{position:absolute;left:0;top:50px;margin:0}\
</style></head><body><div id=\"box\"><button id=\"go\">Start</button><p id=\"status\">Waiting</p></div>\
<script>\
new EventSource('/events');\
document.getElementById('go').addEventListener('click', () => {\
document.getElementById('status').textContent = 'Done'; });\
fetch('/data').then((r) => r.text()).then(() => {\
const ready = document.createElement('div'); ready.id = 'ready';\
document.getElementById('box').append(ready); });\
</script></body></html>";

/// The board of the page as it is after the click: the same markup, drawn by
/// the board's loader.
const BOARD: &str = "<x-dc><helmet><style>\
body{margin:0;background:#123;color:#fff;font:16px Helvetica}\
#box{position:absolute;left:20px;top:20px;width:200px;height:100px}\
button{position:absolute;left:0;top:0;width:120px;height:32px;font:16px Helvetica}\
p{position:absolute;left:0;top:50px;margin:0}\
</style></helmet><div id=\"box\"><button id=\"go\">Start</button><p id=\"status\">Done</p></div></x-dc>";

/// Serves `PAGE` on 127.0.0.1 and answers `/data`; `/events` stays open until
/// the browser closes it. Returns the page's URL.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                let mut request = [0u8; 4096];
                let Ok(read) = stream.read(&mut request) else {
                    return;
                };
                let head = String::from_utf8_lossy(&request[..read]).to_string();
                let target = head.split(' ').nth(1).unwrap_or("/").to_string();
                let (kind, body) = match target.as_str() {
                    "/data" => ("text/plain", "data"),
                    "/events" => ("text/event-stream", ""),
                    "/" => ("text/html; charset=utf-8", PAGE),
                    _ => ("text/plain", ""),
                };
                let length = if kind == "text/event-stream" {
                    String::new()
                } else {
                    format!("Content-Length: {}\r\n", body.len())
                };
                let _ = stream.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\n{length}\r\n{body}")
                        .as_bytes(),
                );
                if kind == "text/event-stream" {
                    // Held open until the browser ends the page.
                    let mut rest = [0u8; 64];
                    while stream.read(&mut rest).is_ok_and(|n| n > 0) {}
                }
            });
        }
    });
    url
}

/// A repo with the board's frame, the fixture of the shot and the board's file.
fn repo(capture: &str) -> (tempfile::TempDir, Declared) {
    let root = fixture::tree(&[(SHOT, BOARD)]);
    fixture::write(
        root.path(),
        "design/mocks/canvas.json",
        &format!("{{\"boards\":{{\"{SHOT}.dc.html\":{{\"x\":0,\"y\":0,\"w\":300,\"h\":200}}}}}}"),
    );
    let file = root.path().join(format!("shots/{SHOT}.json"));
    fixture::write(
        root.path(),
        &format!("shots/{SHOT}.json"),
        &format!("{{\"capture\":{capture},\"stage\":{{\"anything\":1}}}}"),
    );
    let declared = Declared {
        name: name(),
        file,
        marks: Marks::default(),
    };
    (root, declared)
}

/// Takes the shot of the page `serve` gives, with the fixture's `capture`.
fn take(
    root: &Path,
    declared: &Declared,
    deadline: Duration,
) -> (Taken, PathBuf, tempfile::TempDir) {
    let out = tempfile::tempdir().unwrap();
    let app = App::named_for(Platform::Web);
    let settings = Settings::default();
    let folders = fixture::folders();
    let taken = capture(&Job {
        root,
        command: &staging(&serve()),
        declared: std::slice::from_ref(declared),
        app: &app,
        apps: &Apps::default(),
        prefixes: &[],
        base: false,
        out: out.path(),
        settings: &settings,
        folders: &folders,
        deadline,
    })
    .unwrap();
    (taken, out.path().to_path_buf(), out)
}

#[test]
fn a_staged_page_waits_for_its_ready_selector_runs_a_click_and_compares_with_its_board() {
    let (root, declared) = repo("{\"ready\":\"#ready\",\"steps\":[{\"click\":\"#go\"}]}");
    let (taken, out, _keep) = take(root.path(), &declared, Duration::from_secs(120));
    assert!(taken.failed.is_empty(), "{:?}", taken.failed);
    assert_eq!(taken.names, [name()]);
    let shot = Shot::load(&out, &name()).unwrap();
    assert_eq!(shot.problems(&name()), Vec::<String>::new());
    assert_eq!(shot.list.scale, 2.0);
    let texts: Vec<_> = shot
        .list
        .elements
        .iter()
        .filter_map(|e| e.label.clone())
        .collect();
    assert!(texts.iter().any(|t| t == "Done"), "{texts:?}");

    let board = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(fixture::with_browser_at(
            root.path(),
            2.0,
            async |server, chrome| {
                let page = chrome.new_page().await.unwrap();
                let frame = Frame { w: 300, h: 200 };
                let rendered = board::render(&page, SHOT, &server.board_url(SHOT), frame)
                    .await
                    .unwrap();
                let read =
                    render::read_page(&page, &Settings::default(), rendered.frame, Platform::Web)
                        .await
                        .unwrap();
                (rendered, read)
            },
        ));
    let (rendered, read) = board;
    let list = ElementList {
        board: SHOT.to_string(),
        platform: Platform::Web,
        size: crate::shots::Size {
            width: 300.0,
            height: 200.0,
        },
        scale: 2.0,
        elements: read.elements,
        unshot: read.unshot,
        slots: read.slots,
        window: Some(read.window),
    };
    let pair = verify::compare_pair(
        &name(),
        Drawn {
            list,
            picture: Picture::decode(&rendered.png).unwrap(),
            png: PathBuf::new(),
            hash: String::new(),
        },
        &shot,
        out.join(name().file_name("png")),
    );
    assert!(
        pair.findings.is_empty(),
        "{:?}",
        pair.findings
            .iter()
            .map(|f| f.sentence())
            .collect::<Vec<_>>()
    );
    assert!(pair.warnings.is_empty(), "{:?}", pair.warnings);
}

#[test]
fn a_ready_selector_that_matches_nothing_fails_the_shot() {
    let (root, declared) = repo("{\"ready\":\"#never\"}");
    let (taken, _, _keep) = take(root.path(), &declared, Duration::from_secs(20));
    assert!(taken.names.is_empty());
    let [failed] = &taken.failed[..] else {
        panic!("expected one failed shot, found {:?}", taken.failed);
    };
    assert_eq!(failed.name, name());
    assert!(
        failed.why.contains("nothing matches #never"),
        "{}",
        failed.why
    );
}

#[test]
fn a_step_whose_selector_matches_nothing_fails_the_shot() {
    let (root, declared) = repo("{\"ready\":\"#ready\",\"steps\":[{\"click\":\"#missing\"}]}");
    let (taken, _, _keep) = take(root.path(), &declared, Duration::from_secs(120));
    let [failed] = &taken.failed[..] else {
        panic!("expected one failed shot, found {:?}", taken.failed);
    };
    assert!(
        failed.why.contains("click #missing: no element matches it"),
        "{}",
        failed.why
    );
}
