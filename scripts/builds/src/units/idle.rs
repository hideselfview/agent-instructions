//! Deleting incremental folders nothing wrote to lately, for the free-space
//! floor, across every checkout's cargo folders. An incremental folder is
//! only a cache; deleting one costs a recompile of its unit.

use std::collections::HashSet;
use std::path::PathBuf;

use super::{
    INCREMENTAL_DEPTH, holds_any, lock_profile, names_in, newest_mtime, profile_dirs, remove,
};
use crate::fail::Result;
use crate::usage::freeable;

/// An incremental folder, the profile folder it is in, and its newest write
/// anywhere inside.
pub struct Increment {
    pub path: PathBuf,
    pub profile: PathBuf,
    pub newest: f64,
}

/// Every incremental folder in the profile folders of <folders> nothing
/// wrote anywhere inside since <before>, least recently written first. Read
/// without locks; `delete_idle` decides under the profile's lock.
pub fn idle_increments(folders: &[PathBuf], before: f64) -> Result<Vec<Increment>> {
    let mut found = Vec::new();
    for folder in folders {
        for profile in profile_dirs(folder) {
            let incremental = profile.join("incremental");
            if !incremental.is_dir() {
                continue;
            }
            for name in names_in(&incremental)? {
                let path = incremental.join(name);
                if !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                    continue;
                }
                let newest = newest_mtime(&path, INCREMENTAL_DEPTH)?;
                if newest < before {
                    found.push(Increment {
                        path,
                        profile: profile.clone(),
                        newest,
                    });
                }
            }
        }
    }
    found.sort_by(|a, b| {
        a.newest
            .total_cmp(&b.newest)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(found)
}

/// What became of one incremental folder.
pub enum Outcome {
    /// Deleted, freeing this many bytes (not the `deps/` files it hard-links,
    /// which stay with the unit).
    Deleted(u64),
    /// A build holds its profile's cargo lock.
    Locked,
    /// Written since it was listed.
    Written,
    /// A process holds a file in it (<held>, by inode).
    Held,
}

/// Deletes <increment> while holding its profile's cargo lock, unless a
/// build holds that lock, something wrote inside it since <before>, or a
/// process holds a file in it.
pub fn delete_idle(increment: &Increment, before: f64, held: &HashSet<u64>) -> Result<Outcome> {
    let Some(_lock) = lock_profile(&increment.profile)? else {
        return Ok(Outcome::Locked);
    };
    if newest_mtime(&increment.path, INCREMENTAL_DEPTH)? >= before {
        return Ok(Outcome::Written);
    }
    if holds_any(std::slice::from_ref(&increment.path), held)? {
        return Ok(Outcome::Held);
    }
    let size = freeable(&increment.path)?;
    remove(&increment.path)?;
    Ok(Outcome::Deleted(size))
}
