//! flock(2): how builds opens a lock file and takes, or tries to take, its
//! lock. Every lock builds takes on a whole file goes through here.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::fail::{Context, Fail, Result};

/// Takes <operation> (`LOCK_SH` or `LOCK_EX`, with `LOCK_NB` to not wait) on
/// <file>, again when a signal interrupts the wait. Returns false when
/// `LOCK_NB` is given and another process holds a lock that keeps this one
/// out.
pub fn lock_io(file: &File, operation: libc::c_int) -> std::io::Result<bool> {
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

/// `lock_io` on <file>, the file at <path>, failing with the path.
pub fn lock(file: &File, operation: libc::c_int, path: &Path) -> Result<bool> {
    lock_io(file, operation)
        .map_err(|error| Fail::Message(format!("locking {}: {error}", path.display())))
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
