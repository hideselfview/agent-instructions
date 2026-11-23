//! Telling a live daemon from a socket one left behind. Connecting cannot:
//! macOS refuses a connection both when nobody listens and when the listener's
//! backlog is full. A live daemon holds an exclusive lock on its lock file
//! until it exits, and the system drops the lock when the process dies, so the
//! lock says whether a daemon owns the socket. The lock file holds the pid of
//! the daemon that claimed it, so `builds health` knows which process is the
//! dashboard.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::Path;

/// A daemon's claim on its root: held until dropped or the process exits.
pub struct Claim {
    _file: File,
}

/// The lock that serializes acquiring a daemon claim and publishing its PID
/// with readers of that PID. The daemon's claim itself lasts until exit.
fn publication_lock(lock: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock.with_extension("publication"))
}

/// Claims <lock> for this process and writes its pid there. None when a live
/// daemon already holds it.
pub fn claim(lock: &Path) -> io::Result<Option<Claim>> {
    let publication = publication_lock(lock)?;
    publication.lock()?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock)?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)?;
            writeln!(file, "{}", std::process::id())?;
            Ok(Some(Claim { _file: file }))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Waits for a claimant to publish its pid, then reads it. None when no
/// daemon holds <lock> when the reader acquires the publication lock.
pub fn daemon_pid(lock: &Path) -> io::Result<Option<i32>> {
    let publication = match publication_lock(lock) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    publication.lock_shared()?;
    if !daemon_lives(lock)? {
        return Ok(None);
    }
    let text = std::fs::read_to_string(lock)?;
    match text.trim().parse() {
        Ok(pid) => Ok(Some(pid)),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} holds {text:?}, not a pid: {error}", lock.display()),
        )),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `daemon_pid` returns the pid of the daemon that holds the lock, and
    /// None before any daemon claims the lock and after the one holding it
    /// exits. The daemon that exits is another process, as in use: when a
    /// test drops a claim it holds, every child that another test forked
    /// meanwhile still holds the lock until it execs.
    #[test]
    fn the_pid_of_the_daemon_holding_the_lock_is_read_while_it_lives() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        let scratch = crate::testing::Scratch::new("live-pid");
        let lock = scratch.dir.join("lock");
        assert_eq!(daemon_pid(&lock).unwrap(), None);
        let claim = claim(&lock).unwrap().unwrap();
        assert_eq!(daemon_pid(&lock).unwrap(), Some(std::process::id() as i32));
        drop(claim);

        // lockf holds the lock while its shell writes its pid there, says
        // it, and waits for its input to end.
        let mut daemon = Command::new("lockf")
            .arg("-k")
            .arg(&lock)
            .args([
                "/bin/bash",
                "-c",
                "echo $$ > \"$0\"; echo $$; cat > /dev/null",
            ])
            .arg(&lock)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut said = String::new();
        std::io::BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut said)
            .unwrap();
        let pid: i32 = said.trim().parse().unwrap();
        assert_eq!(daemon_pid(&lock).unwrap(), Some(pid));
        drop(daemon.stdin.take());
        assert!(daemon.wait().unwrap().success());
        assert_eq!(daemon_pid(&lock).unwrap(), None);
    }
}
