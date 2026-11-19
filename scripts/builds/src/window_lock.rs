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
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

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

/// The pipe that wakes a wait: the end the takeover's signal handler and a
/// finished lock wait write a byte to, and the end a wait reads. It lasts as
/// long as the process, so a signal that comes at any time finds it open.
static WAKE_WRITE: AtomicI32 = AtomicI32::new(-1);
static WAKE_READ: AtomicI32 = AtomicI32::new(-1);

/// Makes the wake pipe the first time and returns its read end.
fn wake_pipe() -> Result<libc::c_int> {
    static MADE: Mutex<()> = Mutex::new(());
    let _made = MADE.lock().expect("no thread panicked holding it");
    if WAKE_READ.load(Ordering::SeqCst) >= 0 {
        return Ok(WAKE_READ.load(Ordering::SeqCst));
    }
    let mut ends = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        fail!("making the wake pipe: {}", std::io::Error::last_os_error());
    }
    for end in ends {
        unsafe { libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // The writers never wait for room: a full pipe already wakes the reader.
    unsafe { libc::fcntl(ends[1], libc::F_SETFL, libc::O_NONBLOCK) };
    WAKE_READ.store(ends[0], Ordering::SeqCst);
    WAKE_WRITE.store(ends[1], Ordering::SeqCst);
    Ok(ends[0])
}

/// Wakes a wait for a window lock so that it looks at `stopped` again. The
/// takeover's signal handler calls it, so it only does what a signal handler
/// may.
pub fn wake() {
    let end = WAKE_WRITE.load(Ordering::SeqCst);
    if end >= 0 {
        unsafe { libc::write(end, [1u8].as_ptr().cast(), 1) };
    }
}

/// How the thread that waits for a lock on a file has done.
enum Waiting {
    Waits,
    Has,
    Failed(String),
    /// The wait was given up: the thread closes the file when it ends.
    GivenUp,
}

/// Waits for the lock on <file> and takes it, or returns None as soon as
/// <stopped> says a takeover stopped this build. A thread waits in `flock`;
/// the waiter sleeps on the wake pipe, which the thread writes when it has
/// the lock and the takeover's signal handler writes (`wake`). The waiter
/// looks at <stopped> after the pipe is open and before it reads from it, so
/// no takeover goes unseen, and no timer stands in for knowing.
fn wait_for(file: &std::path::Path, stopped: &dyn Fn() -> bool) -> Result<Option<libc::c_int>> {
    let read = wake_pipe()?;
    // Bytes left from an earlier wait or signal wake nothing new.
    loop {
        let mut ready = libc::pollfd {
            fd: read,
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut ready, 1, 0) } <= 0 {
            break;
        }
        let mut byte = 0u8;
        unsafe { libc::read(read, (&mut byte as *mut u8).cast(), 1) };
    }
    let name =
        std::ffi::CString::new(file.as_os_str().as_encoded_bytes()).expect("a path holds no NUL");
    // Left open without close-on-exec, like an app lock: the step's
    // processes hold the lock too.
    let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_CREAT, 0o644) };
    if fd < 0 {
        fail!(
            "opening {}: {}",
            file.display(),
            std::io::Error::last_os_error()
        );
    }
    let state = Arc::new(Mutex::new(Waiting::Waits));
    let waiter = state.clone();
    let shown = file.display().to_string();
    std::thread::spawn(move || {
        let locked = loop {
            if unsafe { libc::flock(fd, libc::LOCK_EX) } == 0 {
                break Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                break Err(format!("locking {shown}: {error}"));
            }
        };
        let mut state = waiter.lock().expect("no thread panicked holding it");
        if matches!(*state, Waiting::GivenUp) {
            unsafe { libc::close(fd) };
            return;
        }
        *state = match locked {
            Ok(()) => Waiting::Has,
            Err(error) => {
                unsafe { libc::close(fd) };
                Waiting::Failed(error)
            }
        };
        wake();
    });
    loop {
        let mut state = state.lock().expect("no thread panicked holding it");
        if stopped() {
            if matches!(*state, Waiting::Has) {
                unsafe { libc::close(fd) };
            } else if !matches!(*state, Waiting::Failed(_)) {
                *state = Waiting::GivenUp;
            }
            return Ok(None);
        }
        match &*state {
            Waiting::Has => return Ok(Some(fd)),
            Waiting::Failed(error) => fail!("{error}"),
            Waiting::Waits | Waiting::GivenUp => {}
        }
        drop(state);
        let mut byte = 0u8;
        unsafe { libc::read(read, (&mut byte as *mut u8).cast(), 1) };
    }
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
            match wait_for(&file, stopped)? {
                Some(lock) => lock,
                None => return Ok(None),
            }
        }
    };
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
