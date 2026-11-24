//! A wait for a file lock that a signal can end, shared by the queue's wait
//! for its turn (queue.rs, SIGUSR1) and the wait for a host's window lock
//! (window_lock.rs, SIGUSR2). Threads blocked in `flock` report through a
//! pipe, and the waiter sleeps in a kqueue that watches the pipe, the signal
//! and the stop signals (lock.rs `STOP_SIGNALS`), which end every wait of a
//! build. The kqueue counts the signals from `Wake::new` on, so a signal is
//! seen whether or not the waiter is blocked at that moment.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::fail;
#[cfg(target_os = "macos")]
use crate::fail::Context;
use crate::fail::Result;
#[cfg(target_os = "macos")]
use crate::kqueue::{Queue, event};
use crate::queue::open_with_mode;

/// What the threads blocked in `flock` and the wait agree on.
pub struct Shared {
    /// The waits whose results are wanted; a thread of another one lets its
    /// ticket go.
    pub current: std::collections::HashSet<u64>,
    /// False once the wait has ended.
    pub open: bool,
}

/// A wait's kqueue: it holds a signal (counted from `new` on) and the read end
/// of the pipe its threads report through.
pub struct Wake {
    #[cfg(target_os = "macos")]
    kqueue: Queue,
    read: libc::c_int,
    write: libc::c_int,
}

/// What `Wake::wait` woke for. Only a report needs reading; the signal alone
/// makes the caller read the order again.
pub struct Woke {
    pub ready: bool,
}

const MESSAGE: usize = 16;

impl Wake {
    /// Off macOS no queue waits: app builds, whose lanes the queue orders,
    /// run only on a Mac.
    #[cfg(not(target_os = "macos"))]
    pub fn new(_signal: libc::c_int) -> Result<Wake> {
        fail!("a wait that a signal can end uses macOS's kqueue, so it runs only on a Mac")
    }

    #[cfg(not(target_os = "macos"))]
    pub fn wait(&self) -> Result<Woke> {
        unreachable!("Wake::new fails off macOS")
    }

    /// Starts a wait for <signal> and the stop signals, counted from now on.
    #[cfg(target_os = "macos")]
    pub fn new(signal: libc::c_int) -> Result<Wake> {
        let mut ends = [0; 2];
        let kqueue =
            Queue::new().context(|| "Builds could not open the lock's event queue".to_string())?;
        if unsafe { libc::pipe(ends.as_mut_ptr()) } < 0 {
            fail!(
                "making a queue to wait in: {}",
                std::io::Error::last_os_error()
            );
        }
        unsafe {
            for end in ends {
                libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            libc::fcntl(ends[0], libc::F_SETFL, libc::O_NONBLOCK);
        }
        let wake = Wake {
            kqueue,
            read: ends[0],
            write: ends[1],
        };
        let mut changes: Vec<libc::kevent> = std::iter::once(signal)
            .chain(crate::lock::STOP_SIGNALS)
            .map(|signal| {
                event(
                    signal as usize,
                    libc::EVFILT_SIGNAL,
                    libc::EV_ADD | libc::EV_CLEAR,
                    0,
                )
            })
            .collect();
        changes.push(event(
            wake.read as usize,
            libc::EVFILT_READ,
            libc::EV_ADD,
            0,
        ));
        wake.kqueue
            .change(&changes)
            .context(|| "Builds could not register the events for the lock wait".to_string())?;
        Ok(wake)
    }

    /// Starts a thread that waits for the lock on <path> and reports the
    /// result for wait <generation>, unless that is no longer the current one.
    pub fn block_on(
        &self,
        path: PathBuf,
        generation: u64,
        mode: libc::c_int,
        shared: Arc<Mutex<Shared>>,
    ) -> Result<()> {
        let write = unsafe { libc::fcntl(self.write, libc::F_DUPFD_CLOEXEC, 0) };
        if write < 0 {
            fail!("starting a wait: {}", std::io::Error::last_os_error());
        }
        std::thread::spawn(move || {
            let result = open_with_mode(&path, 0, mode);
            let code = match &result {
                Ok(fd) => *fd,
                Err(error) => -error.raw_os_error().unwrap_or(libc::EIO),
            };
            let sent = {
                let shared = shared.lock().expect("no panic holding it");
                if shared.open && shared.current.contains(&generation) {
                    let mut message = [0u8; MESSAGE];
                    message[..8].copy_from_slice(&generation.to_le_bytes());
                    message[8..12].copy_from_slice(&code.to_le_bytes());
                    unsafe {
                        libc::write(write, message.as_ptr().cast(), MESSAGE) == MESSAGE as isize
                    }
                } else {
                    false
                }
            };
            unsafe { libc::close(write) };
            if let (false, Ok(fd)) = (sent, result) {
                unsafe { libc::close(fd) };
            }
        });
        Ok(())
    }

    /// Ends the wait: no thread of it reports from now on, and a report not
    /// read yet is let go.
    pub fn end(&self, shared: &Arc<Mutex<Shared>>) {
        shared.lock().expect("no panic holding it").open = false;
        while let Some((_, code)) = self.message() {
            if code >= 0 {
                unsafe { libc::close(code) };
            }
        }
    }

    /// Blocks until a signal came or a thread reported.
    #[cfg(target_os = "macos")]
    pub fn wait(&self) -> Result<Woke> {
        // Room for every signal the kqueue counts and the pipe.
        let mut events = [event(0, 0, 0, 0); 8];
        let ready = self
            .kqueue
            .wait(&mut events, None)
            .context(|| "Builds could not wait for a lock event".to_string())?
            .iter()
            .any(|event| event.filter == libc::EVFILT_READ);
        Ok(Woke { ready })
    }

    /// The next report: the wait it is for and what its thread got, a
    /// descriptor or the negative of an error number.
    pub fn message(&self) -> Option<(u64, i32)> {
        let mut message = [0u8; MESSAGE];
        let read = unsafe { libc::read(self.read, message.as_mut_ptr().cast(), MESSAGE) };
        (read == MESSAGE as isize).then(|| {
            let wait = u64::from_le_bytes(message[..8].try_into().expect("8 bytes"));
            (
                wait,
                i32::from_le_bytes(message[8..12].try_into().expect("4 bytes")),
            )
        })
    }
}

/// The number of the one wait `wait_for_lock` starts.
const LOCK_WAIT: u64 = 1;

/// Waits for the exclusive lock on <file>, which exists, and returns its
/// descriptor (closed on exec), or None as soon as <stopped> says a takeover
/// stopped this build. A thread blocks in `flock` and reports through a pipe,
/// and the waiter sleeps in a kqueue that counts SIGUSR2 and the stop signals
/// from the moment it exists, so a takeover or a stop signal is seen whether
/// it came before or after. A stop signal fails the wait (lock.rs
/// `refuse_when_stopped`), saying the build <waiting_for>.
pub fn wait_for_lock(
    file: &std::path::Path,
    waiting_for: &str,
    stopped: &dyn Fn() -> bool,
) -> Result<Option<libc::c_int>> {
    wait_for_lock_mode(file, waiting_for, stopped, libc::LOCK_EX)
}

pub fn wait_for_lock_mode(
    file: &std::path::Path,
    waiting_for: &str,
    stopped: &dyn Fn() -> bool,
    mode: libc::c_int,
) -> Result<Option<libc::c_int>> {
    let wake = Wake::new(libc::SIGUSR2)?;
    let shared = Arc::new(Mutex::new(Shared {
        current: std::collections::HashSet::from([LOCK_WAIT]),
        open: true,
    }));
    wake.block_on(file.to_path_buf(), LOCK_WAIT, mode, shared.clone())?;
    let taken = (|| -> Result<Option<libc::c_int>> {
        loop {
            if stopped() {
                return Ok(None);
            }
            crate::lock::refuse_when_stopped(&format!("waited for {waiting_for}"))?;
            if wake.wait()?.ready
                && let Some((_, code)) = wake.message()
            {
                if code < 0 {
                    fail!(
                        "locking {}: {}",
                        file.display(),
                        std::io::Error::from_raw_os_error(-code)
                    );
                }
                return Ok(Some(code));
            }
        }
    })();
    // No thread reports from here on, and a report not read is let go.
    wake.end(&shared);
    taken
}

impl Drop for Wake {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.read);
            libc::close(self.write);
        }
    }
}
