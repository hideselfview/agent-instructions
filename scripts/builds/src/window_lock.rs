//! Access to a host's windows and displays. A capture that leases the harness's
//! shared display takes a shared lock. UI tests, runs that take keyboard focus,
//! and runs that can add a display of their own take the lock exclusively.
//! A `builds remote run` job on a Mac is exclusive because builds cannot tell
//! whether its command opens windows.
//!
//! The lock is the file `<root>/.app-windows/<host>`:
//! its descriptor stays open in every process the step starts, so the lock
//! lasts as long as the step's processes do. An exclusive holder writes what
//! it runs to `<host>.run`, for a waiter to name.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::queue;
use crate::text::note;

/// How a step uses the host's windows and displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    /// A window capture that uses the harness's shared display.
    Shared,
    /// A step that can change displays or take the keyboard focus.
    Exclusive,
}

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
    run_file: Option<PathBuf>,
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
        if let Some(run_file) = &self.run_file
            && let Err(error) = std::fs::remove_file(run_file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            note(format!(
                "could not remove {}, so a waiting run may name a run that ended: {error}",
                run_file.display()
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
    take_access(places, host, what, Access::Exclusive, announce, stopped)
}

pub fn take_access(
    places: &Places,
    host: &str,
    what: &str,
    access: Access,
    announce: &dyn Fn(&str),
    stopped: &dyn Fn() -> bool,
) -> Result<Option<Window>> {
    let file = places.app_window_lock_file(host);
    let dir = file.parent().expect("a lock file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let mode = match access {
        Access::Shared => libc::LOCK_SH,
        Access::Exclusive => libc::LOCK_EX,
    };
    let lock = match queue::try_lock_mode(&file, mode)? {
        Some(lock) => lock,
        None => {
            let other =
                holder_of(places, host)?.unwrap_or_else(|| "another window run".to_string());
            announce(&format!("waiting for {other} to release {host}'s display"));
            match crate::lock_wake::wait_for_lock_mode(
                &file,
                &format!("{host}'s display"),
                stopped,
                mode,
            )? {
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
        run_file: (access == Access::Exclusive).then(|| Window::holder_file(places, host)),
    };
    if let Some(run_file) = &window.run_file {
        let holder = Holder {
            pid: std::process::id() as i32,
            what: what.to_string(),
        };
        crate::whole_file::write(
            run_file,
            serde_json::to_vec(&holder).expect("a holder serializes"),
        )?;
    }
    Ok(Some(window))
}

#[cfg(test)]
#[path = "window_lock_tests.rs"]
mod tests;
