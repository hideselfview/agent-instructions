//! Telling a live daemon from a socket one left behind. Connecting cannot:
//! macOS refuses a connection both when nobody listens and when the listener's
//! backlog is full. A live daemon holds an exclusive lock on its lock file
//! until it exits, and the system drops the lock when the process dies, so the
//! lock says whether a daemon owns the socket.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

/// A daemon's claim on its root: held until dropped or the process exits.
pub struct Claim {
    _file: File,
}

/// Claims <lock> for this process. None when a live daemon already holds it.
pub fn claim(lock: &Path) -> io::Result<Option<Claim>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Claim { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Whether a live daemon holds <lock>. A missing lock file means none does.
pub fn daemon_lives(lock: &Path) -> io::Result<bool> {
    let file = match File::open(lock) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error),
    }
}
