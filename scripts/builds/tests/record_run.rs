//! `builds record-run`: the installed program writes a run another program
//! started, so a run that outlasts an install is still recorded.

use std::io::Write;
use std::process::{Command, Stdio};

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

#[test]
fn record_run_writes_a_run_given_as_json_to_the_projects_store() {
    let dir = std::env::temp_dir().join(format!("builds-record-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let env = |command: &mut Command| {
        command
            .env("BUILDS_ROOT", dir.join("volume"))
            .env("HOME", dir.join("home"))
            .env("BUILDS_LISTS", dir.join("lists"));
    };
    let mut init = Command::new(BUILDS);
    init.args(["lists", "init", "demo", "--repos", "demo"]);
    env(&mut init);
    let made = init.output().unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let database = dir.join("lists/demo/builds.sqlite");
    let run = r#"{"started_at":1794700000,"repo":"demo","lane":"Mac","checkout":"side-1","built_ref":"main","who":null,"wait_ms":5,"passed":true,"steps":[{"name":"one","kind":"Step","started_at":1794700001,"wall_ms":10,"cpu_ms":4,"result":"Passed"}]}"#;
    let mut record = Command::new(BUILDS);
    record
        .arg("record-run")
        .arg(&database)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    env(&mut record);
    let mut child = record.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(run.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = Command::new("sqlite3")
        .arg(&database)
        .arg("SELECT checkout || ' ' || passed FROM app_build_runs; SELECT name FROM app_build_steps;")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&rows.stdout), "side-1 1\none\n");
    let _ = std::fs::remove_dir_all(&dir);
}
