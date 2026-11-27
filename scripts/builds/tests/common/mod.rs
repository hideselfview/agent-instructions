//! The one way builds' integration tests start the builds under test
//! (`builds`), with the stand-in system that builds runs the OS's programs
//! from (`system`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The variables a test's builds keeps from the test's own environment: the
/// programs on PATH, the temporary folder, and the Rust toolchain that
/// `builds cargo` runs. Every other variable, such as a caller's session or
/// agent, or a CARGO_* setting of the test run, never reaches it.
const KEPT: [&str; 5] = [
    "PATH",
    "TMPDIR",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "CARGO_HOME",
];

/// The builds under test, run for the test whose folder is <dir> (`command`).
pub fn builds(dir: &Path) -> Command {
    command(env!("CARGO_BIN_EXE_builds"), dir)
}

/// <program> run for the test whose folder is <dir>, in the environment of
/// every builds that the test runs: cleared but `KEPT`, with its home, build
/// volume and stand-in for the machine's temporary folders, folders `home`,
/// `volume` and `temp` in <dir> that this makes, and the stand-in system
/// `system` names. A test sets the variables it needs beyond these on the
/// command.
pub fn command(program: &str, dir: &Path) -> Command {
    for folder in ["home", "volume", "temp"] {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
    }
    let mut command = Command::new(program);
    command.env_clear();
    for name in KEPT {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("HOME", dir.join("home"))
        .env("BUILDS_ROOT", dir.join("volume"))
        .env("BUILDS_TEMP", dir.join("temp"))
        .env("BUILDS_SYSTEM", system(dir));
    command
}

/// The stand-in system of the test whose folder is <dir>, `<dir>/system`:
/// builds runs each program of its table from there, refuses one the folder
/// lacks, and writes each power assertion it takes and releases to the file
/// `power` there, a line each. Unless the test wrote its own, its `lsof` says
/// that no process holds a file open or listens on a port.
pub fn system(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let folder = dir.join("system");
    let lsof = folder.join("lsof");
    if !lsof.exists() {
        std::fs::create_dir_all(&folder).unwrap();
        let writing = folder.join("lsof.writing");
        std::fs::write(&writing, "#!/bin/sh\ncase \"$*\" in *-i*) exit 1;; esac\n").unwrap();
        std::fs::set_permissions(&writing, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&writing, &lsof).unwrap();
    }
    folder
}
