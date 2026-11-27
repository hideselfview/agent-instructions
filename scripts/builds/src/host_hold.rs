//! The loop that holds a build for a host until the host is ready: the mac
//! lane's wait for a host that cannot show windows (host_display.rs) and the
//! wait for Chrome to start (host_chrome.rs) both run it.

use crate::fail::Result;
use crate::lock::Ran;

/// What holds a build, and when the hold began, seconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold<T> {
    pub what: T,
    pub since: i64,
}

/// Holds the build until <held> says the host is ready, and returns true
/// once it does; returns false as soon as <wait> says a takeover stopped the
/// build. <held> says Some(what holds the build) while the host is not
/// ready, and None when it is. Each Some that differs from the last gives
/// <announce> the words <words> makes of it and <record> it with the time
/// the hold began, and <wait> blocks before each next
/// reading. When a hold ends, <announce> gets <over> (one line more) and
/// <record> gets None. A stop signal, or an error of <held>, fails the build;
/// <doing> names what the build waited for.
pub fn hold_until_ready<T: PartialEq>(
    doing: &str,
    held: &dyn Fn() -> Result<Option<T>>,
    words: &dyn Fn(&T) -> String,
    announce: &dyn Fn(&str),
    record: &dyn Fn(Option<Hold<&T>>) -> Result<()>,
    over: &str,
    wait: &dyn Fn() -> Result<Ran>,
) -> Result<bool> {
    let mut said: Option<(T, i64)> = None;
    loop {
        crate::lock::refuse_when_stopped(&format!("waited for {doing}"))?;
        match held()? {
            None => {
                if said.is_some() {
                    announce(over);
                    record(None)?;
                }
                return Ok(true);
            }
            Some(holds) => {
                let since = said
                    .as_ref()
                    .map_or_else(crate::app_timing::now, |(_, since)| *since);
                if said.as_ref().map(|(before, _)| before) != Some(&holds) {
                    announce(&words(&holds));
                    record(Some(Hold {
                        what: &holds,
                        since,
                    }))?;
                }
                said = Some((holds, since));
                if wait()? == Ran::Stopped {
                    return Ok(false);
                }
            }
        }
    }
}
