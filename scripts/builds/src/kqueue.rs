//! Register for kernel events and wait for them, with an optional deadline.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Instant;

pub struct Queue(OwnedFd);

/// A change to an event registration or to an existing user's event.
pub fn event(ident: libc::uintptr_t, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    libc::kevent {
        ident,
        filter,
        flags,
        fflags,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}

impl Queue {
    pub fn new() -> io::Result<Self> {
        // SAFETY: kqueue returns a new descriptor on success.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    pub fn change(&self, changes: &[libc::kevent]) -> io::Result<()> {
        // SAFETY: kevent reads the provided changes and writes no results.
        let result = unsafe {
            libc::kevent(
                self.0.as_raw_fd(),
                changes.as_ptr(),
                changes.len() as libc::c_int,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn wait(&self, deadline: Option<Instant>) -> io::Result<libc::kevent> {
        let mut happened = event(0, 0, 0, 0);
        loop {
            let time = deadline.map(|deadline| {
                let remaining = deadline.saturating_duration_since(Instant::now());
                libc::timespec {
                    tv_sec: remaining.as_secs() as libc::time_t,
                    tv_nsec: remaining.subsec_nanos().into(),
                }
            });
            let timeout = time.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
            // SAFETY: kevent reads no changes and writes at most one event.
            let count = unsafe {
                libc::kevent(
                    self.0.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut happened,
                    1,
                    timeout,
                )
            };
            match count {
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
                0 => return Err(io::ErrorKind::TimedOut.into()),
                _ => return Ok(happened),
            }
        }
    }
}
