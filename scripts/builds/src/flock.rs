//! flock(2): how builds opens a lock file and takes, or tries to take, its
//! lock. Every lock builds takes on a whole file goes through here.

use std::fs::File;
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;

use crate::fail;
use crate::fail::{Context, Fail, Result};

/// Allows a descriptor to stay open across exec. Uses only syscalls, so a
/// caller can set the flag in the forked child before it execs.
pub fn inherit(fd: RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Calls flock(2) with <operation> on <file>, and calls it again when a
/// signal interrupts the wait. Returns false when <operation> has `LOCK_NB`
/// and another process holds a lock that keeps this one out.
fn flock(file: &File, operation: libc::c_int) -> std::io::Result<bool> {
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        match error.kind() {
            std::io::ErrorKind::Interrupted => continue,
            std::io::ErrorKind::WouldBlock => return Ok(false),
            _ => return Err(error),
        }
    }
}

/// Takes <how> (`LOCK_SH` or `LOCK_EX`) on <file>, waiting while another
/// process holds a lock that keeps this one out.
pub fn lock_io(file: &File, how: libc::c_int) -> std::io::Result<()> {
    flock(file, how).map(|_| ())
}

/// Takes <how> (`LOCK_SH` or `LOCK_EX`) on <file>, the file at <path>,
/// waiting while another process holds a lock that keeps this one out.
pub fn lock(file: &File, how: libc::c_int, path: &Path) -> Result<()> {
    lock_io(file, how).map_err(|error| failed(path, error))
}

/// Tries to take <how> (`LOCK_SH` or `LOCK_EX`) on <file>: returns false at
/// once when another process holds a lock that keeps this one out.
pub fn try_lock_io(file: &File, how: libc::c_int) -> std::io::Result<bool> {
    flock(file, how | libc::LOCK_NB)
}

/// `try_lock_io` on <file>, the file at <path>, which its error names.
pub fn try_lock(file: &File, how: libc::c_int, path: &Path) -> Result<bool> {
    try_lock_io(file, how).map_err(|error| failed(path, error))
}

/// The error that says locking the file at <path> failed with <error>.
fn failed(path: &Path, error: std::io::Error) -> Fail {
    Fail::Message(format!("locking {}: {error}", path.display()))
}

/// Opens the lock file at <path> for reading and writing, creates it when it
/// is missing, and never empties it.
pub fn open_creating(path: &Path) -> Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context(|| format!("opening {}", path.display()))
}

/// Takes <how> (`LOCK_SH` or `LOCK_EX`) on the file at <path>, creating it
/// when it is missing, waiting while another process holds a lock that keeps
/// this one out. The lock lasts until the file returned is dropped, and in
/// each process that inherits its descriptor (`inherit`).
pub fn lock_creating(path: &Path, how: libc::c_int) -> Result<File> {
    let file = open_creating(path)?;
    lock(&file, how, path)?;
    Ok(file)
}

/// `lock_creating` without the wait: None while another process holds a lock
/// that keeps this one out.
pub fn try_lock_creating(path: &Path, how: libc::c_int) -> Result<Option<File>> {
    let file = open_creating(path)?;
    Ok(try_lock(&file, how, path)?.then_some(file))
}

/// Whether the path <path> names the open file <file>: a lock file that
/// another process removed, or removed and made again, after this one opened
/// it names another file or none.
pub fn names(file: &File, path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let open = file
        .metadata()
        .context(|| format!("reading {}", path.display()))?;
    match std::fs::metadata(path) {
        Ok(named) => Ok(named.dev() == open.dev() && named.ino() == open.ino()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// Whether a process holds a lock at <path>. A missing path has no lock.
pub fn is_held(path: &Path) -> Result<bool> {
    match File::open(path) {
        Ok(file) => Ok(!try_lock(&file, libc::LOCK_EX, path)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("opening {}: {error}", path.display()),
    }
}
