//! flock(2): how builds opens a lock file and takes, or tries to take, its
//! lock. Every lock builds takes on a whole file goes through here.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::fail::{Context, Fail, Result};

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

/// Tries to take <how> (`LOCK_SH` or `LOCK_EX`) on <file>, the file at
/// <path>: returns false at once when another process holds a lock that
/// keeps this one out.
pub fn try_lock(file: &File, how: libc::c_int, path: &Path) -> Result<bool> {
    flock(file, how | libc::LOCK_NB).map_err(|error| failed(path, error))
}

fn failed(path: &Path, error: std::io::Error) -> Fail {
    Fail::Message(format!("locking {}: {error}", path.display()))
}

/// The lock file at <path>, for reading and writing, made when it is missing
/// and never emptied.
pub fn open_creating(path: &Path) -> Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context(|| format!("opening {}", path.display()))
}
