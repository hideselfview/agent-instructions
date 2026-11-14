//! `builds cargo` and the pruning of unused build copies (`builds units`) on a
//! scratch cargo workspace in a temporary folder, with real cargo builds:
//! cargo_units.sh holds the steps, and this runs them.

use std::process::Command;

#[test]
fn builds_cargo_records_use_and_prune_deletes_only_what_nothing_uses() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/cargo_units.sh");
    let output = Command::new("bash")
        .arg(script)
        .env("BUILDS_BIN", env!("CARGO_BIN_EXE_builds"))
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "ok",
        "{said}"
    );
}
