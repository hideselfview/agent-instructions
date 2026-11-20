//! A clock for how long something took that keeps counting while the
//! machine sleeps.
//!
//! Rust's `Instant` reads a clock that stops while the machine sleeps
//! (CLOCK_UPTIME_RAW on macOS, CLOCK_MONOTONIC on Linux), so a step that a
//! sleep interrupted would record only the time the machine was awake, and
//! the step's recorded end would fall before the moment it really ended. The
//! clock here counts through sleep: macOS's CLOCK_MONOTONIC and Linux's
//! CLOCK_BOOTTIME, each the time since the machine booted. Neither moves
//! when someone sets the wall clock.

use std::time::Duration;

#[cfg(target_os = "macos")]
const COUNTS_SLEEP: libc::clockid_t = libc::CLOCK_MONOTONIC;
#[cfg(target_os = "linux")]
const COUNTS_SLEEP: libc::clockid_t = libc::CLOCK_BOOTTIME;

/// A moment on the clock that counts through sleep.
#[derive(Clone, Copy, Debug)]
pub struct Moment(Duration);

impl Moment {
    pub fn now() -> Moment {
        Moment(since_boot())
    }

    /// The time from this moment to now, sleep included.
    pub fn elapsed(&self) -> Duration {
        since_boot().saturating_sub(self.0)
    }
}

/// The time since the machine booted, sleep included.
fn since_boot() -> Duration {
    let mut time: libc::timespec = unsafe { std::mem::zeroed() };
    // Both clocks exist on every machine builds runs on, so the call does
    // not fail; the assert names the call if it ever does.
    let code = unsafe { libc::clock_gettime(COUNTS_SLEEP, &mut time) };
    assert_eq!(code, 0, "clock_gettime refused the clock that counts sleep");
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}

#[cfg(test)]
#[path = "clock_tests.rs"]
mod tests;
