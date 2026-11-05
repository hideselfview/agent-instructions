//! Trimming a claimed worktree's cargo folder that is over its cap.
//!
//! A claimed worktree's folder is never deleted whole, since its agent is at
//! work in it, but an agent that builds all day would grow it without bound
//! (one coven checkout stood at 50 GB against a 45 GB cap, 37 GB of it
//! incremental folders). An incremental folder is only a cache; deleting one
//! costs a recompile of its unit.

use std::path::PathBuf;

use super::{INCREMENTAL_DEPTH, OpenFiles, holds_any, lock_profile, names_in, newest_mtime, profile_dirs, remove};
use crate::fail::Result;
use crate::text::{counted, human_bytes, note};
use crate::usage::freeable;

/// Deletes incremental folders from each of <folders>, least recently
/// written first (the newest write anywhere inside one), while the whole is
/// over <cap_kib>, starting from <size_kib>. Each profile folder is locked as
/// prune does (a dry run takes no locks), and a folder a process holds a file
/// in stays. Returns the KiB freed.
pub fn trim(cap_kib: u64, size_kib: u64, dry: bool, folders: &[PathBuf]) -> Result<u64> {
    let mut freed = 0;
    let mut left = size_kib as i64;
    for folder in folders {
        let root = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.clone());
        let mut locks = Vec::new();
        let mut candidates: Vec<(f64, PathBuf)> = Vec::new();
        for profile in profile_dirs(&root) {
            if !dry {
                match lock_profile(&profile)? {
                    Some(lock) => locks.push(lock),
                    None => {
                        note(format!("kept the incremental folders in {}: a build holds its lock", profile.display()));
                        continue;
                    }
                }
            }
            let incremental = profile.join("incremental");
            if incremental.is_dir() {
                for name in names_in(&incremental)? {
                    let path = incremental.join(name);
                    let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
                    if is_dir {
                        candidates.push((newest_mtime(&path, INCREMENTAL_DEPTH)?, path));
                    }
                }
            }
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut held = OpenFiles::new(std::slice::from_ref(&root));
        let mut deleted = 0;
        let mut removed: u64 = 0;
        let mut in_use = 0;
        for (_, path) in &candidates {
            if left <= cap_kib as i64 {
                break;
            }
            if holds_any(std::slice::from_ref(path), held.get()?)? {
                in_use += 1;
                continue;
            }
            let size = freeable(path)?;
            if !dry {
                remove(path)?;
            }
            deleted += 1;
            removed += size;
            left -= (size / 1024) as i64;
        }
        freed += removed / 1024;
        if deleted > 0 {
            note(format!(
                "{} {} of {} in {}, least recently written first ({})",
                if dry { "would delete" } else { "deleted" },
                counted(deleted, "incremental folder", "incremental folders"),
                candidates.len(),
                folder.display(),
                human_bytes(removed)
            ));
        }
        if in_use > 0 {
            note(format!(
                "kept {} in {}: a process holds their files",
                counted(in_use, "incremental folder", "incremental folders"),
                folder.display()
            ));
        }
        drop(locks);
    }
    Ok(freed)
}
