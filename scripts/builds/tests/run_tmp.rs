//! A child of `builds cargo` sees a TMPDIR on the build volume, which the end
//! of the run deletes, on a scratch volume with a stand-in for cargo that
//! writes down its environment. The same holds for a shots step of an app
//! build (app_queue.rs, `a_shots_step_sees_a_tmpdir_on_the_build_volume`).

use std::path::PathBuf;
use std::process::Command;

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

#[test]
fn a_child_of_builds_cargo_sees_a_tmpdir_on_the_build_volume_that_the_end_of_the_run_deletes() {
    let dir = PathBuf::from(format!("/tmp/rt-cargo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (volume, bin, work) = (dir.join("volume"), dir.join("bin"), dir.join("work"));
    for folder in [
        &volume,
        &bin,
        &work,
        &dir.join("home"),
        &dir.join("system-tmp"),
    ] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let cargo = bin.join("cargo");
    std::fs::write(
        &cargo,
        r#"#!/bin/sh
echo "$TMPDIR" > "$RECORD"
echo x > "$TMPDIR/tmpdir-file"
echo "$SCCACHE_START_SERVER" > "$RECORD.sccache-mode"
sccache /usr/bin/true > "$RECORD.sccache" 2>&1
test "$?" -ne 0
"#,
    )
    .unwrap();
    std::fs::set_permissions(&cargo, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let record = dir.join("record");
    let socket = dir.join("sccache.sock");
    let path = std::env::var_os("PATH").unwrap();
    let path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path))).unwrap();
    let ran = Command::new(BUILDS)
        .args(["cargo", "check"])
        .current_dir(&work)
        .env("BUILDS_ROOT", &volume)
        .env("HOME", dir.join("home"))
        .env("PATH", path)
        .env("TMPDIR", dir.join("system-tmp"))
        .env("RECORD", &record)
        .env("SCCACHE_START_SERVER", "1")
        .env("SCCACHE_ERROR_LOG", dir.join("server.log"))
        .env("SCCACHE_SERVER_UDS", &socket)
        .env("SCCACHE_DIR", dir.join("cache"))
        .env("SCCACHE_CONF", dir.join("sccache.conf"))
        .env("SCCACHE_IDLE_TIMEOUT", "2")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("BUILDS_AGENT")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&ran.stderr).into_owned();
    assert!(ran.status.success(), "{said}");
    let seen = PathBuf::from(std::fs::read_to_string(&record).unwrap().trim());
    assert_eq!(seen.parent().unwrap(), volume.join(".tmp"), "{said}");
    assert!(!seen.exists(), "the run's folder is still there: {said}");
    let mode = std::fs::read_to_string(dir.join("record.sccache-mode")).unwrap();
    assert_eq!(mode.trim(), "0");
    let sccache = std::fs::read_to_string(dir.join("record.sccache")).unwrap();
    assert!(
        sccache.contains("Cannot open/write log file '/dev/null/local.sccache-server'"),
        "{sccache}"
    );
    assert!(!socket.exists(), "sccache started a scratch server");
    assert_eq!(
        std::fs::read_dir(dir.join("system-tmp")).unwrap().count(),
        0,
        "the run wrote to the system's temporary folder"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
