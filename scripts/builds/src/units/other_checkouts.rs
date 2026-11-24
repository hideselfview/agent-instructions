//! Build script runs that a build folder holds from another checkout.
//!
//! A build script prints `cargo:` lines that cargo hands to every later
//! compile: `rustc-link-search`, `rustc-link-arg`, `rustc-env`. A script that
//! reads a path from its environment, as ffmpeg-sys-next reads FFMPEG_DIR,
//! prints that path, and cargo's record of the run does not name the
//! environment variable, so a folder cloned from another checkout still holds
//! that checkout's path in its printed lines and cargo calls the run up to
//! date. The folder's links then search the other checkout's folders, and fail
//! once that checkout is removed (and read another checkout's files until
//! then). A run whose lines name another checkout is deleted, so cargo runs
//! the script again where it now builds.

use std::path::{Path, PathBuf};

use super::unit::{Unit, units_in};
use super::{profile_dirs, remove};
use crate::checkout::{Checkout, file_name, worktrees_folder};
use crate::evict::lock_exclusive_creating;
use crate::fail::{Context, Result};
use crate::manifest::build_dirs;
use crate::places::Places;

/// Deletes the fingerprint of each build script run in the cargo folders of
/// <checkout>'s build root that printed a `cargo:` line naming another
/// checkout of the repo (`OtherCheckouts`), so that cargo runs the script
/// again. A folder cloned before builds forgot such runs at the clone holds
/// them still.
pub fn forget_runs_naming_other_checkouts(places: &Places, checkout: &Checkout) -> Result<()> {
    let others = OtherCheckouts::of(places, checkout);
    let root = checkout.build_root(places);
    for dir in build_dirs(&checkout.top)? {
        others.forget_runs_in(&root.join(&dir.path))?;
    }
    Ok(())
}

/// The paths of a repo's checkouts other than one of them: any folder below
/// the repo's worktrees folder or the repo's build roots but the checkout's
/// own, and the main checkout's own folder. A removed checkout's paths are
/// named the same way as a live one's.
pub struct OtherCheckouts {
    /// The folder holding the repo's worktrees, and the name of the
    /// checkout's own folder there (None for a checkout outside it).
    worktrees: Option<(PathBuf, Option<String>)>,
    /// The folder holding the repo's build roots, and the checkout's own.
    builds: (PathBuf, String),
    /// The main checkout's folder, unless it holds the checkout.
    main: Option<PathBuf>,
}

impl OtherCheckouts {
    pub fn of(places: &Places, checkout: &Checkout) -> OtherCheckouts {
        OtherCheckouts {
            worktrees: worktrees_folder(&checkout.main).map(|folder| {
                let own = (checkout.top.parent() == Some(folder.as_path()))
                    .then(|| file_name(&checkout.top));
                (folder, own)
            }),
            builds: (places.root.join(checkout.repo()), checkout.name()),
            main: (!checkout.top.starts_with(&checkout.main)).then(|| checkout.main.clone()),
        }
    }

    /// Deletes the fingerprint of each build script run in <folder>'s
    /// profiles whose `cargo:` lines name another checkout, while holding
    /// each profile's cargo lock, as a build does, so no cargo build of the
    /// folder reads a run as it goes.
    pub fn forget_runs_in(&self, folder: &Path) -> Result<()> {
        for profile in profile_dirs(folder)? {
            let _lock = lock_exclusive_creating(&profile.join(".cargo-lock"))?;
            for unit in units_in(&profile)?.list {
                if let Some(fingerprint) = &unit.fingerprint
                    && self.printed_by(&unit)?
                {
                    remove(fingerprint)?;
                }
            }
        }
        Ok(())
    }

    fn printed_by(&self, unit: &Unit) -> Result<bool> {
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
            .any(|line| self.named_by(line)))
    }

    fn named_by(&self, line: &[u8]) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let inside = |folder: &Path, own: Option<&str>| {
            let mut prefix = folder.as_os_str().as_bytes().to_vec();
            prefix.push(b'/');
            line.windows(prefix.len())
                .enumerate()
                .filter(|(_, window)| *window == prefix.as_slice())
                .any(|(at, _)| {
                    let name = &line[at + prefix.len()..];
                    let name = &name[..name.iter().take_while(|b| continues_name(**b)).count()];
                    !name.is_empty() && Some(name) != own.map(str::as_bytes)
                })
        };
        self.worktrees
            .as_ref()
            .is_some_and(|(folder, own)| inside(folder, own.as_deref()))
            || inside(&self.builds.0, Some(&self.builds.1))
            || self.main.as_ref().is_some_and(|main| {
                let main = main.as_os_str().as_bytes();
                line.windows(main.len()).enumerate().any(|(at, window)| {
                    window == main
                        && line
                            .get(at + main.len())
                            .is_none_or(|b| !continues_name(*b))
                })
            })
    }
}

/// Does <byte> go on a folder's name, so side-1 is not found in side-15?
fn continues_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}
