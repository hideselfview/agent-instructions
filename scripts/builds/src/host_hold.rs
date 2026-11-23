//! The loop that holds a build for a host until the host is ready: the mac
//! lane's wait for a locked screen (host_screen_lock.rs) and the wait for
//! Chrome to start (host_chrome.rs) both run it.

use crate::fail::Result;
use crate::lock::Ran;

/// Holds the build until <held> says the host is ready, and returns true
/// once it does; returns false as soon as <wait> says a takeover stopped the
/// build. <held> says Some(words) while the host is not ready, and None when
/// it is. The first Some gives <announce> its words and runs <first>, and
/// <wait> blocks before each next reading. When a hold ends, <announce> gets
/// <over> (one line more). A stop signal, or an error of <held>, fails the
/// build; <doing> names what the build waited for.
pub fn hold_until_ready(
    doing: &str,
    held: &dyn Fn() -> Result<Option<String>>,
    announce: &dyn Fn(&str),
    first: &dyn Fn() -> Result<()>,
    over: &str,
    wait: &dyn Fn() -> Result<Ran>,
) -> Result<bool> {
    let mut waited = false;
    loop {
        crate::lock::refuse_when_stopped(&format!("waited for {doing}"))?;
        match held()? {
            None => {
                if waited {
                    announce(over);
                }
                return Ok(true);
            }
            Some(words) => {
                if !waited {
                    announce(&words);
                    first()?;
                    waited = true;
                }
                if wait()? == Ran::Stopped {
                    return Ok(false);
                }
            }
        }
    }
}
