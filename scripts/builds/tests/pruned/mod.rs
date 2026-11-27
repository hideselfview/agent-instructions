//! A volume that a prune just finished on, for the integration tests that
//! start a dashboard.

use std::path::Path;

/// Records that a `builds prune` finished just now on the volume at
/// <volume>, so a dashboard started on it runs no prune while the test runs:
/// a prune would also delete what it finds in this Mac's temporary folders.
pub fn pruned_now(volume: &Path) {
    let dir = volume.join(".builds-daemon");
    std::fs::create_dir_all(&dir).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(dir.join("pruned"), format!("{now}\n")).unwrap();
}
