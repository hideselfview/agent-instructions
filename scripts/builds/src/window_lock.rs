//! The lock on a host's display. A step that opens windows (a test run of an
//! `opens-windows` line, or a shots run that passes `--windows`) drives the
//! apps on the host's one display, and a second such run there would make
//! XCUITest see the other run's windows as interruptions. So each of those
//! steps holds the host's window lock while it runs, whichever repo, lane and
//! app checkout it runs in, and a step that finds the lock held waits for it
//! and says which run holds it.
//!
//! The lock is the file `<root>/.app-windows/<host>`, held like an app lock:
//! its descriptor stays open in every process the step starts, so the lock
//! lasts as long as the step's processes do. The holder writes what it runs to
//! `<host>.run` while it holds the lock, for a waiter to name.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

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
        let _ = std::fs::remove_file(&self.run_file);
        unsafe { libc::close(self.lock) };
    }
}

/// Whether process <pid> runs.
fn runs(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// What the run holding <host>'s window lock runs, when there is one that
/// still lives.
fn holder_of(places: &Places, host: &str) -> Option<String> {
    let text = std::fs::read(Window::holder_file(places, host)).ok()?;
    let holder: Holder = serde_json::from_slice(&text).ok()?;
    runs(holder.pid).then_some(holder.what)
}

/// Takes <host>'s window lock for the step <what> describes, waiting while
/// another run holds it, and says on stderr which run it waits for. Returns
/// None when <stopped> says a takeover stopped this build while it waited.
pub fn take(
    places: &Places,
    host: &str,
    what: &str,
    stopped: &dyn Fn() -> bool,
) -> Result<Option<Window>> {
    let file = places.app_window_lock_file(host);
    let dir = file.parent().expect("a lock file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let mut told: Option<String> = None;
    loop {
        if let Some(lock) = queue::try_lock(&file)? {
            let run_file = Window::holder_file(places, host);
            let holder = Holder {
                pid: std::process::id() as i32,
                what: what.to_string(),
            };
            let window = Window { lock, run_file };
            crate::whole_file::write(
                &window.run_file,
                serde_json::to_vec(&holder).expect("a holder serializes"),
            )?;
            return Ok(Some(window));
        }
        if stopped() {
            return Ok(None);
        }
        if let Some(other) = holder_of(places, host)
            && told.as_deref() != Some(&other)
        {
            note(format!(
                "waiting for {other} to end, since only one run that opens windows takes {host}'s display at a time"
            ));
            told = Some(other);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
#[path = "window_lock_tests.rs"]
mod tests;
