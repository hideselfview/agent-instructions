//! The folder of this builds' version, for the integration tests whose app
//! builds run lanes.

use std::path::Path;

/// Makes the folder of this builds' version in <home>, with the builds under
/// test, as the first app build of the version makes it (programs.rs).
pub fn programs_folder(home: &Path) {
    let dir = home.join(".local/lib/builds").join(env!("BUILDS_SOURCE"));
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_builds"), dir.join("builds")).unwrap();
}
