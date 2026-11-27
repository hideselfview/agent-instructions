//! The loop that holds a build for a host until the host is ready: the mac
//! lane's wait for a host that cannot show windows (host_display.rs) and the
//! wait for Chrome to start (host_chrome.rs) both run it.

use std::ops::ControlFlow;

use crate::fail::Result;
use crate::lock::Ran;

/// What holds a build, and when the hold began, seconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold<T> {
    pub what: T,
    pub since: i64,
}

/// Holds the build until <held> says the host is ready, and returns what
/// <held> gave with that; returns None as soon as <wait> says a takeover
/// stopped the build. <held> says `Continue(what holds the build)` while the
/// host is not ready, and `Break` when it is. Each `Continue` that differs
/// from the last gives <announce> the words <words> makes of it and <record>
/// it with the time the hold began, and <wait> blocks before each next
/// reading. When a hold ends, <announce> gets <over> (one line more) and
/// <record> gets None. A stop signal, or an error of <held>, fails the build;
/// <doing> names what the build waited for.
pub fn hold_until_ready<T: PartialEq, R>(
    doing: &str,
    held: &dyn Fn() -> Result<ControlFlow<R, T>>,
    words: &dyn Fn(&T) -> String,
    announce: &dyn Fn(&str),
    record: &dyn Fn(Option<Hold<&T>>) -> Result<()>,
    over: &str,
    wait: &dyn Fn() -> Result<Ran>,
) -> Result<Option<R>> {
    let mut said: Option<(T, i64)> = None;
    loop {
        crate::lock::refuse_when_stopped(&format!("waited for {doing}"))?;
        match held()? {
            ControlFlow::Break(ready) => {
                if said.is_some() {
                    announce(over);
                    record(None)?;
                }
                return Ok(Some(ready));
            }
            ControlFlow::Continue(holds) => {
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
                    return Ok(None);
                }
            }
        }
    }
}
