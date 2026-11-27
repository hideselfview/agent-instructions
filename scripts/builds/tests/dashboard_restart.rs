#![cfg(target_os = "macos")]

use std::fs::{self, File};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

mod common;

#[test]
fn the_dashboard_reports_readiness_to_installs_startup_connection() {
    let dir = PathBuf::from(format!("/private/tmp/builds-ready-{}", std::process::id()));
    let volume = dir.join("volume");
    common::pruned_now(&volume);
    let log = dir.join("startup.log");
    let (mut installer, dashboard) = UnixStream::pair().unwrap();
    installer
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_builds"))
        .args(["dashboard", "--port", "0"])
        .env("BUILDS_ROOT", &volume)
        .env("BUILDS_POWER_RECORD", dir.join("power"))
        .env("BUILDS_TEMP", dir.join("temp"))
        .env("BUILDS_LISTS", dir.join("lists"))
        .env("BUILDS_DASHBOARD_RESTART", "1")
        .stdin(Stdio::from(OwnedFd::from(dashboard)))
        .stdout(Stdio::null())
        .stderr(File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let mut message = [0];
    let received = installer.read_exact(&mut message);
    let stopped = child.kill();
    let ended = child.wait();
    let diagnostic = fs::read_to_string(log).unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(received.is_ok(), "{received:?}, {diagnostic}");
    assert_eq!(message, [1], "{diagnostic}");
    stopped.unwrap();
    ended.unwrap();
}
