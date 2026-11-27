//! What keeps a process that a test of app builds started from outliving the
//! test, whether the test passes or fails.

use std::ops::{Deref, DerefMut};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// A builds command that a test started. Dropping it while it runs sends it
/// SIGTERM, which builds passes to the steps it runs, and waits for it to end.
pub struct Spawned(Child);

impl Spawned {
    pub fn start(command: &mut Command) -> Spawned {
        Spawned(command.spawn().unwrap())
    }
}

impl Deref for Spawned {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl DerefMut for Spawned {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        // A child the test already waited for has no pid to signal.
        if matches!(self.0.try_wait(), Ok(None)) {
            unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
            let _ = self.0.wait();
        }
    }
}

/// How long `wait_for_free_lanes` waits for one lock.
const PATIENCE: Duration = Duration::from_secs(60);

/// Waits until no process holds a lock of an app lane in <repo>, a repo's
/// folder on a build volume. A step whose lane or lock holder the test killed
/// runs on until it ends, and holds the lock until then. Fails the test when a
/// lock stays held, and only says so when the test has already failed.
pub fn wait_for_free_lanes(repo: &Path) {
    let folders = std::fs::read_dir(repo).into_iter().flatten().flatten();
    let locks = folders
        .filter(|folder| folder.file_name().to_string_lossy().ends_with(".locks"))
        .flat_map(|folder| std::fs::read_dir(folder.path()).unwrap().flatten());
    for lock in locks {
        let path = lock.path();
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let deadline = Instant::now() + PATIENCE;
        while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            if Instant::now() > deadline {
                let said = format!("a process the test started still holds {}", path.display());
                assert!(std::thread::panicking(), "{said}");
                eprintln!("{said}");
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
