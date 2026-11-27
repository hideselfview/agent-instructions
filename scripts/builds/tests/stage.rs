//! A whole `builds dashboard stage` run: it prints the page's address as its
//! first line, serves the page for the shot, and ends when its stdin closes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FIXTURE: &str = r##"{
  "capture": { "ready": "#x", "steps": [] },
  "stage": {
    "now": 1700000000,
    "state": {
      "version": 1, "generated_at": 1700000000, "root": "/v",
      "volume": { "free_kib": 10, "floor_kib": 5, "hard_floor_kib": 2, "size_kib": 3 },
      "load": null, "health": null, "health_error": null, "hosts": [],
      "compile_cache": [], "compile_cache_error": null, "repos": []
    },
    "lists": { "projects": [] },
    "hooks": { "projects": [] }
  }
}"##;

fn folder(name: &str, fixture: &str) -> PathBuf {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-stage-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("web.demo.page.json"), fixture).unwrap();
    dir
}

fn stage(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_builds"));
    command
        .args(["dashboard", "stage", "--fixtures"])
        .arg(dir)
        .arg("web.demo.page")
        .current_dir(dir)
        .env("BUILDS_ROOT", dir.join("no-volume"))
        .env("BUILDS_POWER_RECORD", dir.join("power"))
        .env("BUILDS_TEMP", dir.join("temp"))
        .env("HOME", dir.join("home"))
        .env("BUILDS_LISTS", dir.join("lists"))
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("BUILDS_AGENT");
    command
}

#[test]
fn the_stage_prints_its_address_serves_the_page_and_ends_with_its_stdin() {
    let dir = folder("serves", FIXTURE);
    let mut child = stage(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut url = String::new();
    stdout.read_line(&mut url).unwrap();
    let url = url.trim_end();
    let address = url
        .strip_prefix("http://")
        .and_then(|rest| rest.strip_suffix('/'))
        .unwrap_or_else(|| panic!("the first line is no page address: {url:?}"));
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "GET / HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    drop(child.stdin.take());
    let ended = child.wait_with_output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        ended.status.success(),
        "{}",
        String::from_utf8_lossy(&ended.stderr)
    );
}

#[test]
fn a_fixture_of_the_wrong_shape_stops_the_stage_before_it_prints_an_address() {
    let dir = folder(
        "wrong",
        &FIXTURE.replace("\"size_kib\": 3", "\"size_kib\": \"three\""),
    );
    let ended = stage(&dir).stdin(Stdio::piped()).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!ended.status.success());
    assert_eq!(String::from_utf8_lossy(&ended.stdout), "");
    let error = String::from_utf8_lossy(&ended.stderr);
    assert!(error.contains("web.demo.page.json"), "{error}");
}
