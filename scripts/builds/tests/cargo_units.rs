//! `builds cargo` and the pruning of unused build copies (`builds units`) on a
//! scratch cargo workspace in a temporary folder, with real cargo builds:
//! cargo_units.sh holds the steps, and this runs them with a cargo that writes
//! each of cargo's two build folder layouts.

mod common;

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
    let dir =
        std::env::temp_dir().join(format!("builds-test-units-{}-{layout}", std::process::id()));
    // The steps check that pruning keeps what a process holds open, which
    // only this Mac's lsof sees; lsof only reads.
    std::fs::create_dir_all(dir.join("system")).unwrap();
    std::os::unix::fs::symlink("/usr/sbin/lsof", dir.join("system/lsof")).unwrap();
    // The script runs the builds under test, in the environment every
    // test's builds gets.
    let mut command = common::command("bash", &dir);
    let builds = common::builds(&dir).get_program().to_owned();
    // The links `builds install` makes in a home, through which `builds
    // cargo` compiles and links.
    let bin = dir.join("home/.local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    for link in ["builds-rustc", "builds-linker"] {
        std::os::unix::fs::symlink(&builds, bin.join(link)).unwrap();
    }
    command
        .arg(script)
        .env("BUILDS_BIN", &builds)
        .env("TOOLCHAIN", toolchain)
        .env("LAYOUT", layout);
    if let Some(older) = older {
        command.env("OLDER_TOOLCHAIN", older);
    }
    let output = command.output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
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
