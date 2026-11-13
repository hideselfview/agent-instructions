//! The trash: <root>/.trash on the build volume, where an eviction renames a
//! folder into a slot of its own and removes the slot.
//!
//! Three kinds of process touch it at once (evictions, the floor, prune), so
//! no two ever work on the same tree:
//!
//! - <root>/.trash.lock, beside the trash. An eviction holds it shared from
//!   making its slot until the folder is renamed into the slot and, for the
//!   background removal, until `rm` is started. An emptier holds it exclusive
//!   only to rename the whole `.trash` to a new `.trash-emptying.XXXXXXXX`
//!   folder at the root: it never takes a slot mid-eviction, and an eviction
//!   after that makes a fresh `.trash`.
//! - Each slot is locked while it is being removed (an flock on the folder; a
//!   background `rm` inherits it, so it lasts as long as the `rm`). An
//!   emptier removes a slot only after taking that lock, so it never works
//!   in a tree another process is removing: it waits for that removal (the
//!   floor) or leaves the slot to it (prune's background removal).
//! - A `.trash-emptying.*` folder a killed emptier left is emptied by the
//!   next emptier the same way. It holds only unreferenced bytes.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::{Places, EMPTYING_PREFIX};

/// The shared trash lock: waits only for an emptier's rename.
pub fn lock_shared(places: &Places) -> Result<File> {
    crate::evict::lock_shared_creating(&places.trash_lock())
}

fn lock_exclusive(places: &Places) -> Result<File> {
    let path = places.trash_lock();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .context(|| format!("opening {}", path.display()))?;
    flock(&file, libc::LOCK_EX, &path)?;
    Ok(file)
}

fn flock(file: &File, operation: i32, path: &Path) -> Result<bool> {
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        match error.kind() {
            std::io::ErrorKind::Interrupted => {}
            std::io::ErrorKind::WouldBlock => return Ok(false),
            _ => fail!("locking {}: {error}", path.display()),
        }
    }
}

/// <dir> opened, with its exclusive lock taken: waiting for it, or None
/// when <wait> is false and another process holds it.
fn lock_folder(dir: &Path, wait: bool) -> Result<Option<File>> {
    let file = File::open(dir).context(|| format!("opening {}", dir.display()))?;
    let operation = if wait { libc::LOCK_EX } else { libc::LOCK_EX | libc::LOCK_NB };
    Ok(flock(&file, operation, dir)?.then_some(file))
}

/// A slot in the trash, locked while it is removed. Made while the caller
/// holds the shared trash lock.
pub struct Slot {
    pub path: PathBuf,
    hold: File,
}

impl Slot {
    /// Makes the trash if it is missing and a slot in it.
    pub fn make(places: &Places) -> Result<Slot> {
        let trash = places.trash();
        std::fs::create_dir_all(&trash).context(|| format!("creating {}", trash.display()))?;
        let path = crate::evict::make_temp_dir(&trash, "evict.")?;
        let hold = lock_folder(&path, true)?.expect("a lock waited for is taken");
        Ok(Slot { path, hold })
    }

    /// Removes the slot before returning. A removal that cannot finish (see
    /// `remove_or_warn`) warns and leaves the slot in the trash, whose next
    /// emptier removes it: nothing wrong is left, only space unreclaimed.
    pub fn remove_now(self) {
        remove_or_warn(&self.path);
    }

    /// Starts `rm -rf` on the slot; it keeps the slot's lock until it ends.
    pub fn remove_background(self) -> Result<()> {
        spawn_rm(std::slice::from_ref(&self.path), None, &[&self.hold])
    }
}

/// Removes <path> and everything in it, once; a path already gone is
/// removed. When that fails, says so on stderr and goes on: what is left is
/// bytes nothing references, in the trash, for the next emptier. Returns
/// whether <path> is gone.
pub fn remove_or_warn(path: &Path) -> bool {
    match std::fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            crate::text::note(format!(
                "warning: could not finish removing {}: {error}; it stays in the trash for the next emptier",
                path.display()
            ));
            false
        }
    }
}

/// `rm -rf <paths>` in a session of its own, so it finishes after this
/// process and its terminal are gone, then `rmdir <then_rmdir>` (failing
/// harmlessly while something is left in it). The files in <holds> stay open
/// in it, so their locks last as long as it runs.
fn spawn_rm(paths: &[PathBuf], then_rmdir: Option<&Path>, holds: &[&File]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("rm -rf -- \"$@\"; [ -n \"$RMDIR\" ] && rmdir -- \"$RMDIR\" 2>/dev/null; exit 0")
        .arg("rm")
        .args(paths)
        .env("RMDIR", then_rmdir.unwrap_or(Path::new("")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let fds: Vec<i32> = holds.iter().map(|file| file.as_raw_fd()).collect();
    unsafe {
        command.pre_exec(move || {
            libc::setsid();
            for fd in &fds {
                libc::fcntl(*fd, libc::F_SETFD, 0);
            }
            Ok(())
        });
    }
    command.spawn().context(|| format!("starting rm -rf in {}", paths.first().map_or(String::new(), |p| p.display().to_string())))?;
    Ok(())
}

/// Renames the trash aside to a new `.trash-emptying.*` folder, under the
/// exclusive lock, and returns it with every other `.trash-emptying.*` folder
/// there: the folders to empty.
fn take_out(places: &Places) -> Result<Vec<PathBuf>> {
    let _exclusive = lock_exclusive(places)?;
    let trash = places.trash();
    if trash.is_dir() {
        let aside = crate::evict::make_temp_dir(&places.root, EMPTYING_PREFIX)?;
        std::fs::rename(&trash, &aside).context(|| format!("moving {} aside to {}", trash.display(), aside.display()))?;
    }
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(&places.root).context(|| format!("listing {}", places.root.display()))? {
        let entry = entry.context(|| format!("listing {}", places.root.display()))?;
        if entry.file_name().to_string_lossy().starts_with(EMPTYING_PREFIX) && entry.path().is_dir() {
            folders.push(entry.path());
        }
    }
    Ok(folders)
}

fn entries(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let listing = match std::fs::read_dir(folder) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(error) => fail!("listing {}: {error}", folder.display()),
    };
    for entry in listing {
        found.push(entry.context(|| format!("listing {}", folder.display()))?.path());
    }
    Ok(found)
}

/// Empties the trash before returning: each slot once no other process is
/// removing it.
pub fn empty_now(places: &Places) -> Result<()> {
    for folder in take_out(places)? {
        for slot in entries(&folder)? {
            if slot.is_dir() {
                // Waits for a removal already running. Its slot is then gone.
                match lock_folder(&slot, true) {
                    Ok(_held) => {
                        remove_or_warn(&slot);
                    }
                    Err(_) if !slot.exists() => {}
                    Err(error) => return Err(error),
                }
            } else {
                if let Err(error) = std::fs::remove_file(&slot)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    crate::text::note(format!("warning: could not remove {}: {error}", slot.display()));
                }
            }
        }
        // A slot a warning above left keeps its folder; the next emptier
        // takes both.
        if let Err(error) = std::fs::remove_dir(&folder)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::text::note(format!("warning: could not remove {}: {error}", folder.display()));
        }
    }
    Ok(())
}

/// Starts emptying the trash in the background. A slot another process is
/// removing is left to it.
pub fn empty_background(places: &Places) -> Result<()> {
    for folder in take_out(places)? {
        let mut paths = Vec::new();
        let mut holds = Vec::new();
        for slot in entries(&folder)? {
            if slot.is_dir() {
                match lock_folder(&slot, false) {
                    Ok(Some(hold)) => {
                        holds.push(hold);
                        paths.push(slot);
                    }
                    Ok(None) => {}
                    Err(_) if !slot.exists() => {}
                    Err(error) => return Err(error),
                }
            } else {
                paths.push(slot);
            }
        }
        let held: Vec<&File> = holds.iter().collect();
        spawn_rm(&paths, Some(&folder), &held)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "trash_tests.rs"]
mod tests;
