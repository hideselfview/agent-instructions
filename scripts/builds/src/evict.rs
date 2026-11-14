//! Deleting a folder whole, unless a build is using it.
//!
//! A build in another process can start at any moment, so "in use" is
//! decided while holding what a build holds. Cargo keeps an exclusive flock on
//! <target>/<profile>/.cargo-lock (and <target>/<triple>/<profile>/.cargo-lock)
//! while it compiles: those locks are taken without waiting, and a cargo
//! already holding one makes the folder in use. While they are held, no cargo
//! can start writing; a cargo that queues on one gets it after the rename and
//! writes through the link into the new, empty folder. Folders without cargo
//! locks (Xcode's DerivedData, Gradle, dotnet, lake) count as in use while any
//! process holds a file or working directory inside them; that is checked
//! last, immediately before the rename. A folder inside a checkout's build
//! root is in use too while a `builds cargo` build holds that checkout's
//! guard (`units::GUARD`): cargo lets go of its lock before `cargo test`
//! runs the tests it built. The guard is taken, creating its file when a
//! checkout has had no `builds cargo` build yet, so a build starting after the
//! check waits for the guard until the folder has been moved, never uses a
//! folder judged idle.

use std::ffi::CString;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;

/// How the folder's space comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Removed by a background `rm -rf` after the rename.
    Background,
    /// Removed before `evict` returns, for a caller that reads the free space
    /// next.
    Now,
}

/// Whether an empty folder goes back where the deleted one was, so a link to
/// it stays valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recreate {
    Yes,
    No,
}

/// Deletes <dir> whole unless a build is using it: renames it into the trash
/// (instant; see `trash`) and removes it as <removal> says. The trash is
/// <root>/.trash on the build volume, and for a folder on another volume a new
/// folder beside it, since a rename can't cross volumes. Returns false, deleting nothing,
/// when the folder is in use.
pub fn evict(places: &Places, dir: &Path, recreate: Recreate, removal: Removal) -> Result<bool> {
    let folder = std::fs::canonicalize(dir).context(|| format!("resolving {}", dir.display()))?;
    let parent = folder
        .parent()
        .expect("a folder to delete has a parent")
        .to_path_buf();
    let same_volume = device(&parent)? == device(&places.root)?;
    let _guard = match crate::units::lock_guard_creating(&folder)? {
        crate::units::Guard::Busy => return Ok(false),
        guard => guard,
    };
    let Some(_locks) = cargo_locks(&folder)? else {
        return Ok(false);
    };
    if held_by_a_process(&folder)? {
        return Ok(false);
    }
    let name = folder.file_name().expect("a folder to delete has a name");
    if !same_volume {
        let slot = make_temp_dir(&parent, ".builds-deleting.")?;
        let moved = move_in(&folder, &slot, name, recreate)?;
        drop(_locks);
        remove(&slot, removal)?;
        return Ok(moved);
    }
    // The shared trash lock keeps an emptier from renaming the trash away
    // between making the slot and the folder reaching it, or from emptying
    // the slot while it is being removed.
    let _trash_lock = crate::trash::lock_shared(places)?;
    let slot = crate::trash::Slot::make(places)?;
    let moved = move_in(&folder, &slot.path, name, recreate)?;
    drop(_locks);
    match removal {
        Removal::Now => slot.remove_now(),
        Removal::Background => slot.remove_background()?,
    }
    Ok(moved)
}

/// Renames <folder> into <slot>: false, moving nothing, when another process
/// renamed it away first. Two processes can pick the same folder (a DerivedData
/// entry has no lock of its own), and the rename decides which one deletes it.
pub(crate) fn move_in(
    folder: &Path,
    slot: &Path,
    name: &std::ffi::OsStr,
    recreate: Recreate,
) -> Result<bool> {
    match std::fs::rename(folder, slot.join(name)) {
        Ok(()) => {}
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && !folder.exists()
                && slot.is_dir() =>
        {
            return Ok(false);
        }
        Err(error) => {
            // The folder stays where it was, as if it were in use.
            crate::text::note(format!(
                "warning: could not move {} into {}: {error}; it was not deleted",
                folder.display(),
                slot.display()
            ));
            return Ok(false);
        }
    }
    if recreate == Recreate::Yes {
        std::fs::create_dir(folder).context(|| format!("recreating {}", folder.display()))?;
    }
    Ok(true)
}

fn device(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)
        .context(|| format!("reading {}", path.display()))?
        .dev())
}

/// Every .cargo-lock in <folder> and two levels below, each taken without
/// waiting; None as soon as one is held by a build.
pub(crate) fn cargo_locks(folder: &Path) -> Result<Option<Vec<File>>> {
    let mut held = Vec::new();
    let mut pending = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => fail!("listing {}: {error}", dir.display()),
        };
        for entry in entries {
            let entry = entry.context(|| format!("listing {}", dir.display()))?;
            let kind = entry
                .file_type()
                .context(|| format!("reading {}", entry.path().display()))?;
            if entry.file_name() == ".cargo-lock" && kind.is_file() {
                match try_lock(&entry.path())? {
                    Some(file) => held.push(file),
                    None => return Ok(None),
                }
            } else if kind.is_dir() && depth < 2 {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    Ok(Some(held))
}

/// Takes the exclusive flock on <path> without waiting: the open file, or
/// None while another process holds it.
pub fn try_lock(path: &Path) -> Result<Option<File>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .context(|| format!("opening {}", path.display()))?;
    flock_now(file, path)
}

/// Like `try_lock`, creating the file when it is missing (cargo's own
/// profile lock, the package cache's).
pub fn try_lock_creating(path: &Path) -> Result<Option<File>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context(|| format!("opening {}", path.display()))?;
    flock_now(file, path)
}

/// Takes a shared flock on <path>, creating it when missing, waiting while
/// another process holds it exclusively.
pub fn lock_shared_creating(path: &Path) -> Result<File> {
    lock_creating_waiting(path, libc::LOCK_SH)
}

/// Takes the exclusive flock on <path>, creating it when missing, waiting
/// while any other process holds it.
pub fn lock_exclusive_creating(path: &Path) -> Result<File> {
    lock_creating_waiting(path, libc::LOCK_EX)
}

fn lock_creating_waiting(path: &Path, operation: libc::c_int) -> Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context(|| format!("opening {}", path.display()))?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            fail!("locking {}: {error}", path.display());
        }
    }
}

fn flock_now(file: File, path: &Path) -> Result<Option<File>> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(None);
    }
    fail!("locking {}: {error}", path.display())
}

/// Does any process but this one hold a file, a working directory or a
/// mapped file at or inside <folder>? (`lsof -n -Fpn`)
pub fn held_by_a_process(folder: &Path) -> Result<bool> {
    let me = std::process::id().to_string();
    let output = Command::new("lsof")
        .args(["-n", "-Fpn"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context(|| "running lsof".to_string())?;
    let folder = folder.as_os_str().as_bytes();
    let mut pid: &[u8] = b"";
    for line in output.stdout.split(|byte| *byte == b'\n') {
        match line.first() {
            Some(b'p') => pid = &line[1..],
            Some(b'n') if pid != me.as_bytes() => {
                let name = &line[1..];
                if name == folder
                    || (name.starts_with(folder) && name.get(folder.len()) == Some(&b'/'))
                {
                    return Ok(true);
                }
            }
            _ => {}
        }
    }
    Ok(false)
}

/// A new folder in <dir> named <prefix> and eight random characters.
pub fn make_temp_dir(dir: &Path, prefix: &str) -> Result<PathBuf> {
    let template = dir.join(format!("{prefix}XXXXXXXX"));
    let template = CString::new(template.as_os_str().as_bytes()).expect("a path holds no NUL");
    let mut bytes = template.into_bytes_with_nul();
    let made = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
    if made.is_null() {
        fail!(
            "creating a folder in {}: {}",
            dir.display(),
            std::io::Error::last_os_error()
        );
    }
    bytes.pop();
    Ok(PathBuf::from(std::ffi::OsString::from(
        std::ffi::OsStr::from_bytes(&bytes),
    )))
}

/// Removes <slot> now (warning, and leaving it, when it cannot be finished), or starts `rm -rf` on it in a session of its own, so
/// it finishes after this process and its terminal are gone.
pub fn remove(slot: &Path, removal: Removal) -> Result<bool> {
    match removal {
        Removal::Now => {
            return Ok(crate::trash::remove_or_warn(slot));
        }
        Removal::Background => {
            background_rm(slot)?;
        }
    }
    Ok(true)
}

/// `rm -rf <path>` detached from this process.
pub fn background_rm(path: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new("rm");
    command
        .arg("-rf")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command
        .spawn()
        .context(|| format!("starting rm -rf {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "evict_tests.rs"]
mod tests;
