//! What keeps a process that a test of app builds started from outliving the
//! test, whether the test passes or fails.

use std::io::ErrorKind;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::{Child, Command};
use std::sync::mpsc::channel;
use std::time::Duration;

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
    let folders = std::fs::read_dir(repo)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|folder| folder.file_name().to_string_lossy().ends_with(".locks"));
    let locks = folders.flat_map(|folder| std::fs::read_dir(folder.path()).unwrap());
    for lock in locks {
        let path = lock.unwrap().path();
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            // No process can newly lock a file that is gone from its folder.
            Err(error) if error.kind() == ErrorKind::NotFound => {
                eprintln!("not waiting for {}: {error}", path.display());
                continue;
            }
            Err(error) => panic!("opening {}: {error}", path.display()),
        };
        let (taken, wait) = channel();
        let waited_for = path.clone();
        std::thread::spawn(move || {
            if taken.send(file.lock()).is_err() {
                eprintln!(
                    "{} came free after the wait for it ended",
                    waited_for.display()
                );
            }
        });
        match wait.recv_timeout(PATIENCE) {
            Ok(locked) => locked.unwrap(),
            Err(error) => {
                let said = format!(
                    "a process the test started still holds {} ({error})",
                    path.display()
                );
                assert!(std::thread::panicking(), "{said}");
                eprintln!("{said}");
            }
        }
    }
}
