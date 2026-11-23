//! The lock on a host's display. A step that opens windows (a test run of an
//! `opens-windows` line, or a shots run that passes `--windows`) drives the
//! apps on the host's one display, and a second such run there would make
//! XCUITest see the other run's windows as interruptions. So each of those
//! steps holds the host's window lock while it runs, whichever repo, lane and
//! app checkout it runs in, and a step that finds the lock held waits for it
//! and says which run holds it. A `builds remote run` job on a Mac holds the
//! same lock (remote.rs `run_job_on`), since builds cannot tell whether its
//! command opens windows.
//!
//! The lock is the file `<root>/.app-windows/<host>`:
//! its descriptor stays open in every process the step starts, so the lock
//! lasts as long as the step's processes do. The holder writes what it runs to
//! `<host>.run` while it holds the lock, for a waiter to name.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::queue;
use crate::text::note;

/// What the run that holds a host's window lock runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Holder {
    pid: i32,
    /// "<repo>'s <checkout>, step <step>".
    what: String,
}

/// A host's window lock, held until this value drops.
pub struct Window {
    lock: libc::c_int,
    run_file: PathBuf,
}

impl Window {
    fn holder_file(places: &Places, host: &str) -> PathBuf {
        places
            .app_window_lock_file(host)
            .with_file_name(format!("{host}.run"))
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // Removed while the lock is still held, so a waiter never reads the
        // record of a run that has ended.
        if let Err(error) = std::fs::remove_file(&self.run_file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            note(format!(
                "could not remove {}, so a waiting run may name a run that ended: {error}",
                self.run_file.display()
            ));
        }
        unsafe { libc::close(self.lock) };
    }
}

/// What the run holding <host>'s window lock runs, when there is one that
/// still lives.
fn holder_of(places: &Places, host: &str) -> Result<Option<String>> {
    let file = Window::holder_file(places, host);
    let text = match std::fs::read(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("reading {}: {error}", file.display()),
    };
    let holder: Holder = match serde_json::from_slice(&text) {
        Ok(holder) => holder,
        Err(error) => fail!("reading {}: {error}", file.display()),
    };
    Ok(crate::procs::runs(holder.pid).then_some(holder.what))
}

/// Takes <host>'s window lock for the step <what> describes. While another
/// run holds it, `take` gives <announce> the words that name that run, then
/// waits for the lock to free. Returns None when <stopped> says a takeover
/// stopped this build while it waited.
pub fn take(
    places: &Places,
    host: &str,
    what: &str,
    announce: &dyn Fn(&str),
    stopped: &dyn Fn() -> bool,
) -> Result<Option<Window>> {
    let file = places.app_window_lock_file(host);
    let dir = file.parent().expect("a lock file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let lock = match queue::try_lock(&file)? {
        Some(lock) => lock,
        None => {
            let other = holder_of(places, host)?.unwrap_or_else(|| "another run".to_string());
            announce(&format!(
                "waiting for {other} to end, since only one run that opens windows takes {host}'s display at a time"
            ));
            match crate::lock_wake::wait_for_lock(&file, &format!("{host}'s display"), stopped)? {
                Some(lock) => lock,
                None => return Ok(None),
            }
        }
    };
    // Keep the display locked while the step's processes still use it.
    if let Err(error) = crate::flock::inherit(lock) {
        unsafe { libc::close(lock) };
        fail!(
            "Builds could not pass the lock at {} to the step: {error}",
            file.display()
        );
    }
    let window = Window {
        lock,
        run_file: Window::holder_file(places, host),
    };
    let holder = Holder {
        pid: std::process::id() as i32,
        what: what.to_string(),
    };
    crate::whole_file::write(
        &window.run_file,
        serde_json::to_vec(&holder).expect("a holder serializes"),
    )?;
    Ok(Some(window))
}

#[cfg(test)]
#[path = "window_lock_tests.rs"]
mod tests;
