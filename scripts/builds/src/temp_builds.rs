//! Build output in the temporary folders, which belongs on the build volume.
//!
//! A build pointed at a temporary folder (CARGO_TARGET_DIR=/tmp/...,
//! xcodebuild -derivedDataPath /tmp/..., a probe crate or package built where
//! it was made) writes 5-14 GB there that nothing deletes: before `builds`
//! covered them, such folders piled up past 100 GB in /private/tmp. Three
//! kinds are recognized by what their tool writes at their top:
//!
//!   a cargo target folder         .rustc_info.json
//!   an Xcode DerivedData folder   info.plist beside Logs, and Build or
//!                                 ModuleCache.noindex
//!   a SwiftPM folder              workspace-state.json beside checkouts,
//!                                 repositories or artifacts
//!
//! At the top of each temporary folder, entries named claude-*, com.apple.*
//! and tmux-* are skipped whole: Claude Code sessions' scratch folders (pruned
//! by their own rule), launchd's per-session socket folders, and the tmux
//! server's socket folder, whose deletion strands a running server. Only
//! folders the caller owns are listed, and links are never followed.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::Result;
use crate::places::Places;
use crate::trees::{is_swiftpm_folder, is_xcode_tree};

/// Deep enough for /private/tmp/<probe>/<crate>/target and
/// $TMPDIR/<tool>-<id>/<package>/.build; a build folder's own contents are
/// never walked.
const MAX_DEPTH: usize = 4;
const SKIPPED_PREFIXES: [&str; 3] = ["claude-", "com.apple.", "tmux-"];

/// The temporary folders build output can land in: /private/tmp, the user's
/// temporary folder, and this session's TMPDIR when it is another, unless it
/// is inside a Claude Code session's folder (/private/tmp/claude-*), which is
/// skipped whole.
pub fn temp_folders(places: &Places) -> Vec<PathBuf> {
    let mut folders = vec![places.private_tmp.clone()];
    if let Some(user) = &places.user_temp {
        folders.push(user.clone());
    }
    if let Some(tmp) = &places.tmpdir {
        let text = tmp.to_string_lossy();
        let trimmed = PathBuf::from(text.trim_end_matches('/'));
        let skipped = matches!(
            text.as_ref(),
            "/tmp" | "/tmp/" | "/private/tmp" | "/private/tmp/"
        ) || text.starts_with("/tmp/claude-")
            || text.starts_with("/private/tmp/claude-");
        if !skipped && Some(&trimmed) != places.user_temp.as_ref() {
            folders.push(trimmed);
        }
    }
    folders
}

fn is_build_folder(names: &HashSet<String>) -> bool {
    names.contains(".rustc_info.json") || is_xcode_tree(names) || is_swiftpm_folder(names)
}

/// Every build output folder found under the temporary folders, outermost
/// only.
pub fn temp_build_folders(places: &Places) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for top in temp_folders(places) {
        let Ok(top) = std::fs::canonicalize(&top) else {
            continue;
        };
        if top.is_dir() {
            walk(&top, &top, 0, places.uid, &mut found)?;
        }
    }
    Ok(found)
}

fn walk(top: &Path, dir: &Path, depth: usize, uid: u32, found: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // os.walk skips what it can't list: other users' folders, ones gone
        // since.
        Err(error) if unreadable(&error) => return Ok(()),
        Err(error) => fail!("listing build output in {}: {error}", dir.display()),
    };
    let mut names = HashSet::new();
    let mut dirs = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            dirs.push(name.clone());
        }
        names.insert(name);
    }
    if dir != top && is_build_folder(&names) {
        found.push(dir.to_path_buf());
        return Ok(());
    }
    if depth >= MAX_DEPTH {
        return Ok(());
    }
    dirs.sort();
    for name in dirs {
        if dir == top
            && SKIPPED_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        let child = dir.join(&name);
        match std::fs::symlink_metadata(&child) {
            Ok(meta) if meta.uid() == uid => walk(top, &child, depth + 1, uid, found)?,
            _ => {}
        }
    }
    Ok(())
}

fn unreadable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::NotADirectory
    )
}
