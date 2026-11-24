//! Build script runs that a cloned build folder holds from another checkout.
//!
//! A build script prints `cargo:` lines that cargo hands to every later
//! compile: `rustc-link-search`, `rustc-link-arg`, `rustc-env`. A script that
//! reads a path from its environment, as ffmpeg-sys-next reads FFMPEG_DIR,
//! prints that path, and cargo's record of the run does not name the
//! environment variable, so a clone made in another checkout still holds the
//! source's path in its printed lines and cargo calls the run up to date. The
//! clone's links then search the source checkout's folders, and fail once
//! that checkout is removed (and read another checkout's files until then).

use std::path::Path;

use super::unit::{Unit, units_in};
use super::{profile_dirs, remove};
use crate::fail::{Context, Result};

/// Deletes the fingerprint of each build script run in <folder>, a cloned
/// build folder, that printed a `cargo:` line naming one of <paths> (the
/// checkout the clone came from: its working tree and its build root), so that
/// cargo runs the script again in the new checkout.
pub fn forget_runs_naming(folder: &Path, paths: &[&Path]) -> Result<()> {
    for profile in profile_dirs(folder)? {
        for unit in units_in(&profile)?.list {
            if let Some(fingerprint) = &unit.fingerprint
                && prints_a_path_of(&unit, paths)?
            {
                remove(fingerprint)?;
            }
        }
    }
    Ok(())
}

fn prints_a_path_of(unit: &Unit, paths: &[&Path]) -> Result<bool> {
    let Some(printed) = unit.printed() else {
        return Ok(false);
    };
    let bytes = match std::fs::read(&printed) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).context(|| format!("reading {}", printed.display()));
        }
    };
    Ok(bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| line.starts_with(b"cargo:"))
        .any(|line| paths.iter().any(|path| names(line, path))))
}

/// Does <line> hold <path> as a whole path: not followed by a character that
/// goes on a folder's name, so side-1 is not found in side-15?
fn names(line: &[u8], path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let path = path.as_os_str().as_bytes();
    if path.is_empty() {
        return false;
    }
    line.windows(path.len()).enumerate().any(|(at, window)| {
        window == path
            && line.get(at + path.len()).is_none_or(|next| {
                !(next.is_ascii_alphanumeric() || matches!(next, b'_' | b'-' | b'.'))
            })
    })
}
