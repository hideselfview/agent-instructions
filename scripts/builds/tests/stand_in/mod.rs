//! The programs of a test's stand-in system (`common::system`), for the
//! integration tests whose builds runs some of them.

use std::path::{Path, PathBuf};

/// Writes the program <name> into the stand-in system <folder>, a shell
/// script whose body is <body>, and returns its path. It is written to
/// another name and moved into place, so no builds runs it while it is still
/// open for writing.
pub fn stand_in(folder: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let program = folder.join(name);
    let writing = folder.join(format!("{name}.writing"));
    std::fs::write(&writing, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&writing, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&writing, &program).unwrap();
    program
}

/// Writes into the stand-in system <folder> an `xcodebuild` that says it is
/// Xcode 26.3, so a test never depends on the Xcode the Mac it runs on has
/// installed or has agreed to the license of.
pub fn xcode(folder: &Path) {
    stand_in(
        folder,
        "xcodebuild",
        "if [ \"$1\" = -version ]; then echo 'Xcode 26.3'; echo 'Build version 17C529'; else exit 1; fi",
    );
}
