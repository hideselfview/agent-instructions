//! The folder a test of builds makes in its test run's temporary folder: each
//! integration test's, and a unit test's where its scratch folder will not do.

use std::path::PathBuf;

/// The path of a folder named <name> for this test's process, in the
/// temporary folder of its test run, with nothing there yet. `builds cargo`
/// gives each run a temporary folder on the build volume, which no prune
/// walks (.config/nextest.toml refuses a write to the machine's own). A unix
/// socket's path has room for about 100 bytes, so <name> is short.
pub fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("{name}-{}", std::process::id()));
    match std::fs::remove_dir_all(&dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            panic!("removing {}: {error}", dir.display())
        }
        _ => dir,
    }
}
