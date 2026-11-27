//! A host where Chrome cannot start. A shots run draws the boards it compares
//! with Chrome, so a build with shots on a host whose Chrome does not start
//! builds the app and then fails at the comparison, and each retry builds
//! the app again. On a Mac, Chrome's main thread waits for the window server
//! to answer before it prints the address of its debugging socket, and when
//! the window server does not answer, Chrome never prints it.
//!
//! - Before an app build with shots builds anything on a host, it starts
//!   Chrome there (`builds shots chrome`, which prints one line that `parse`
//!   reads) and holds the build while Chrome prints no address, saying so in
//!   the build's output, naming the host (`wait_until_starts`). The build
//!   keeps its checkout while it holds, and records the step `STEP` beside
//!   its lock with the hold, which `builds health` names (host_display.rs
//!   `findings`).

use crate::fail;
use crate::fail::Result;
use crate::host::{Host, place};
use crate::host_display::{Wait, WaitReason};
use crate::host_hold::Hold;
use crate::lock::Ran;

/// The step the held build records while it waits for Chrome.
pub const STEP: &str = "start Chrome";

/// What `builds shots chrome` says about starting Chrome on a host.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    Started,
    /// Chrome ran and printed no debugging address in its deadline.
    NoAddress(String),
    /// Chrome did not start for another reason.
    Failed(String),
}

/// Reads the line `builds shots chrome` printed in <said>: `chrome=started
/// <version>`, `chrome=no-address <why>` or `chrome=failed <why>`. Fails,
/// quoting <said>, when it holds none of them.
pub fn parse(said: &str) -> std::result::Result<Start, String> {
    let line = crate::host::answer_value(said, "chrome")
        .ok_or_else(|| format!("`builds shots chrome` printed no chrome= line: {said:?}"))?;
    let (kind, rest) = line.split_once(' ').unwrap_or((line, ""));
    match kind {
        "started" => Ok(Start::Started),
        "no-address" => Ok(Start::NoAddress(rest.to_string())),
        "failed" => Ok(Start::Failed(rest.to_string())),
        _ => Err(format!("`builds shots chrome` printed {line:?}")),
    }
}

/// Holds the build until Chrome starts on <host>, and returns true once it
/// does. <start> starts Chrome there; while it says Chrome printed no
/// address, each new answer gives <announce> the words that name the host
/// and <record> the hold (`WaitReason::ChromeCannotStart`) with the time it
/// began, and <pause> waits before the next try; when Chrome starts,
/// <record> gets None, so that the build's record names no step until the
/// next one starts. It returns false as soon as <pause> says a takeover
/// stopped the build, and fails when Chrome fails to start for another
/// reason or <start> cannot ask. The end of a hold gives <announce> one line
/// more.
pub fn wait_until_starts(
    host: &Host,
    start: &dyn Fn() -> std::result::Result<Start, String>,
    announce: &dyn Fn(&str),
    record: &dyn Fn(Option<Hold<Wait>>) -> Result<()>,
    pause: &dyn Fn() -> Result<Ran>,
) -> Result<bool> {
    let name = &host.name;
    crate::host_hold::hold_until_ready(
        &format!("Chrome to start on {name}"),
        &|| match start() {
            Ok(Start::Started) => Ok(None),
            Ok(Start::NoAddress(why)) => Ok(Some(why)),
            Ok(Start::Failed(why)) => fail!("Chrome cannot start on {}: {why}", place(name)),
            Err(error) => fail!("{error}"),
        },
        &|why| {
            format!(
                "waiting for Chrome to start on {}, since a shots run draws its boards with Chrome and builds nothing there until it does; on a Mac this means the window server does not answer: {why}",
                place(name)
            )
        },
        announce,
        &|holds| {
            record(holds.map(|hold| Hold {
                what: Wait::on(
                    host,
                    vec![WaitReason::ChromeCannotStart {
                        why: hold.what.clone(),
                    }],
                ),
                since: hold.since,
            }))
        },
        &format!("Chrome starts on {} again", place(name)),
        pause,
    )
}

#[cfg(test)]
#[path = "host_chrome_tests.rs"]
mod tests;
