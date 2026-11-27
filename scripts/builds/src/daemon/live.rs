//! Telling a live daemon from a socket one left behind. Connecting cannot:
//! macOS refuses a connection both when nobody listens and when the listener's
//! backlog is full. A live daemon holds an exclusive lock on its lock file
//! until it exits, and the system drops the lock when the process dies, so the
//! lock says whether a daemon owns the socket. The lock file holds the pid of
//! the daemon that claimed it, so `builds health` knows which process is the
//! dashboard.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::flock;

/// A daemon's claim on its root: held until dropped or the process exits.
pub struct Claim {
    _file: File,
}

/// A dashboard's lifetime lock and the lock that protects publication of its PID.
pub struct DashboardLock<'a> {
    path: &'a Path,
    #[cfg(test)]
    observe: Option<&'a dyn Fn(Publication<'_>)>,
}

#[cfg(test)]
enum Publication<'a> {
    Claimed,
    Reading(&'a File),
    ReadLocked,
}

impl<'a> DashboardLock<'a> {
    pub fn new(path: &'a Path) -> Self {
        Self {
            path,
            #[cfg(test)]
            observe: None,
        }
    }

    /// Serializes acquiring the claim and publishing its PID with readers.
    /// The daemon's claim itself lasts until exit.
    fn publication_lock(&self) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("publication"))
    }

    #[cfg(test)]
    fn observe(&self, event: Publication<'_>) {
        if let Some(observe) = self.observe {
            observe(event);
        }
    }

    /// Claims this lock for this process and writes its PID there. None
    /// when a live daemon already holds the lock.
    pub fn claim(&self) -> io::Result<Option<Claim>> {
        let publication = self.publication_lock()?;
        flock::lock_io(&publication, libc::LOCK_EX)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path)?;
        if !flock::try_lock_io(&file, libc::LOCK_EX)? {
            return Ok(None);
        }
        #[cfg(test)]
        self.observe(Publication::Claimed);
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Some(Claim { _file: file }))
    }

    /// Waits for a claimant to publish its PID, then reads it. None when
    /// no daemon holds the claim when the reader acquires the publication lock.
    pub fn pid(&self) -> io::Result<Option<i32>> {
        let publication = match self.publication_lock() {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        #[cfg(test)]
        self.observe(Publication::Reading(&publication));
        flock::lock_io(&publication, libc::LOCK_SH)?;
        #[cfg(test)]
        self.observe(Publication::ReadLocked);
        if !daemon_lives(self.path)? {
            return Ok(None);
        }
        let text = std::fs::read_to_string(self.path)?;
        match text.trim().parse() {
            Ok(pid) => Ok(Some(pid)),
            Err(error) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} holds {text:?}, not a pid: {error}", self.path.display()),
            )),
        }
    }
}

/// Whether a live daemon holds <lock>. A missing lock file means none does.
pub fn daemon_lives(lock: &Path) -> io::Result<bool> {
    let file = match File::open(lock) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(!flock::try_lock_io(&file, libc::LOCK_SH)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::TryLockError;

    #[test]
    fn a_reader_waits_until_the_claimant_has_replaced_the_previous_pid() {
        use std::sync::mpsc;
        use std::time::Duration;

        let scratch = crate::testing::Scratch::new("claim-publication");
        let path = scratch.dir.join("lock");
        let pid = std::process::id() as i32;
        let previous = if pid == 1 { 2 } else { 1 };
        std::fs::write(&path, format!("{previous}\n")).unwrap();

        let (claimed, claim_is_held) = mpsc::channel();
        let (publish, can_publish) = mpsc::channel();
        let (published, publication_finished) = mpsc::channel();
        let patience = Duration::from_secs(30);
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            let before_publish = |_: Publication<'_>| {
                claimed.send(()).unwrap();
                can_publish.recv_timeout(patience).unwrap();
            };
            let lock = DashboardLock {
                path: &writer_path,
                observe: Some(&before_publish),
            };
            let claim = lock.claim().unwrap().unwrap();
            published.send(()).unwrap();
            claim
        });
        claim_is_held.recv_timeout(patience).unwrap();

        let observe_read = |event: Publication<'_>| match event {
            Publication::Reading(reader) => {
                assert!(matches!(
                    reader.try_lock_shared(),
                    Err(TryLockError::WouldBlock)
                ));
                publish.send(()).unwrap();
            }
            Publication::ReadLocked => {
                // The writer has released its lock. A separate file
                // can now only be refused by the reader's shared lock.
                publication_finished.recv_timeout(patience).unwrap();
                let probe = DashboardLock::new(&path).publication_lock().unwrap();
                assert!(matches!(probe.try_lock(), Err(TryLockError::WouldBlock)));
            }
            Publication::Claimed => unreachable!("the reader does not claim the dashboard"),
        };
        let lock = DashboardLock {
            path: &path,
            observe: Some(&observe_read),
        };
        let owner = lock.pid().unwrap();
        let _claim = writer.join().unwrap();
        assert_eq!(owner, Some(pid));
    }

    /// The PID reader returns the pid of the daemon that holds the lock, and
    /// None before any daemon claims the lock and after the one holding it
    /// exits. The daemon that exits is another process, as in use: when a
    /// test drops a claim it holds, every child that another test forked
    /// meanwhile still holds the lock until it execs.
    #[test]
    fn the_pid_of_the_daemon_holding_the_lock_is_read_while_it_lives() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        let scratch = crate::testing::Scratch::new("live-pid");
        let path = scratch.dir.join("lock");
        let lock = DashboardLock::new(&path);
        assert_eq!(lock.pid().unwrap(), None);
        let claim = lock.claim().unwrap().unwrap();
        assert_eq!(lock.pid().unwrap(), Some(std::process::id() as i32));
        drop(claim);

        // lockf holds the lock while its shell writes its pid there, says
        // it, and waits for its input to end.
        let mut daemon = Command::new("lockf")
            .arg("-k")
            .arg(&path)
            .args([
                "/bin/bash",
                "-c",
                "echo $$ > \"$0\"; echo $$; cat > /dev/null",
            ])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut said = String::new();
        std::io::BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut said)
            .unwrap();
        let pid: i32 = said.trim().parse().unwrap();
        assert_eq!(lock.pid().unwrap(), Some(pid));
        drop(daemon.stdin.take());
        assert!(daemon.wait().unwrap().success());
        assert_eq!(lock.pid().unwrap(), None);
    }
}
