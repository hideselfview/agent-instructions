//! Integration checks for the sccache server probe.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

#[test]
fn the_tmpdir_probe_preserves_spaces_equals_and_newlines_in_a_process_path() {
    const CHILD: &str = "BUILDS_TEST_TMPDIR_PROBE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        eprintln!("ready");
        std::io::stdin().read_line(&mut String::new()).unwrap();
        return;
    }

    let root = PathBuf::from(format!("/tmp/rt-probe-{}", std::process::id()));
    let missing = root.join("cache FLAG=value\nrest");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "the_tmpdir_probe_preserves_spaces_equals_and_newlines_in_a_process_path",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("TMPDIR", &missing)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    let report = Command::new(BUILDS)
        .args(["sccache-tmpdir", &child.id().to_string()])
        .env("BUILDS_ROOT", &root)
        .env("BUILDS_POWER_RECORD", root.join("power"))
        .output();
    drop(child.stdin.take());
    let status = child.wait().unwrap();

    assert_eq!(ready.trim(), "ready");
    assert!(status.success());
    let report = report.unwrap();
    assert!(report.status.success(), "{report:?}");
    let issue: String = serde_json::from_slice(&report.stdout).unwrap();
    assert!(issue.contains(&format!("{missing:?}")), "{issue}");
}
