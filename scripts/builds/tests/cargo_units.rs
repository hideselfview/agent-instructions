//! `builds cargo` and the pruning of unused build copies (`builds units`) on a
//! scratch cargo workspace in a temporary folder, with real cargo builds:
//! cargo_units.sh holds the steps, and this runs them with a cargo that writes
//! each of cargo's two build folder layouts.

use std::process::Command;

/// The toolchain whose cargo writes shared folders (.fingerprint beside
/// deps), the layout of every cargo before 1.100.
const SHARED_FOLDERS: &str = "1.95.0";
/// The toolchain whose cargo writes unit folders (build/<package>/<hash>/),
/// the layout of every cargo from 1.100 on.
const UNIT_FOLDERS: &str = "1.100.0";

/// Runs cargo_units.sh with <toolchain>'s cargo, which writes <layout>, and
/// with <older>'s too when given.
fn run(toolchain: &str, layout: &str, older: Option<&str>) {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/cargo_units.sh");
    let mut command = Command::new("bash");
    command
        .arg(script)
        .env("BUILDS_BIN", env!("CARGO_BIN_EXE_builds"))
        .env("TOOLCHAIN", toolchain)
        .env("LAYOUT", layout)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID");
    if let Some(older) = older {
        command.env("OLDER_TOOLCHAIN", older);
    }
    let output = command.output().unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "ok",
        "{said}"
    );
}

#[test]
fn builds_cargo_records_use_and_prune_deletes_only_what_nothing_uses_in_shared_folders() {
    run(SHARED_FOLDERS, "shared", None);
}

#[test]
fn builds_cargo_records_use_and_prune_deletes_only_what_nothing_uses_in_unit_folders() {
    run(UNIT_FOLDERS, "unit-folders", Some(SHARED_FOLDERS));
}
