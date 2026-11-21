//! The folder of this builds' version's programs, for the integration tests
//! whose app builds run lanes.

use std::path::Path;

/// Makes the folder of this builds' version's programs in <home>, as the
/// first app build of the version makes it (programs.rs), with the builds
/// under test and a builds-shots that prints nothing: the builds under test
/// has no builds-shots beside it to copy.
pub fn programs_folder(home: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let dir = home.join(".local/lib/builds").join(env!("BUILDS_SOURCE"));
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_builds"), dir.join("builds")).unwrap();
    let shots = dir.join("builds-shots");
    std::fs::write(&shots, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&shots, std::fs::Permissions::from_mode(0o755)).unwrap();
}
