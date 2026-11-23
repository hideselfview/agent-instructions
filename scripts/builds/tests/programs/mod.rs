//! The folder of this builds' version, for the integration tests whose app
//! builds run lanes.

use std::path::{Path, PathBuf};

/// Makes the folder of this builds' version in <home>, with the builds under
/// test, as the first app build of the version makes it (programs.rs).
/// Returns the path it gave the executable.
pub fn programs_folder(home: &Path) -> PathBuf {
    let dir = home.join(".local/lib/builds").join(env!("BUILDS_SOURCE"));
    std::fs::create_dir_all(&dir).unwrap();
    let builds = dir.join("builds");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_builds"), &builds).unwrap();
    builds
}
