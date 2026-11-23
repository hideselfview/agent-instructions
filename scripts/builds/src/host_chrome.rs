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
//!   keeps its checkout while it holds.
//! - `builds health` names each host that holds a build that way, from the
//!   step the held build records beside its lock (`findings`).

use crate::fail;
use crate::fail::Result;
use crate::health::{Advice, LockHealth};
use crate::host::place;
use crate::lock::Ran;

/// What the held build records as its step while it waits for Chrome, ahead
/// of the host's name.
pub const HOLD_STEP: &str = "waiting for Chrome to start on";

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

/// Holds the build until Chrome starts on host <name>, and returns true once
/// it does. <start> starts Chrome there; while it says Chrome printed no
/// address, the first such answer gives <announce> the words that name the
/// host and <hold> the step the build records, and <pause> waits before the
/// next try. It returns false as soon as <pause> says a takeover stopped the
/// build, and fails when Chrome fails to start for another reason or <start>
/// cannot ask. The end of a hold gives <announce> one line more.
pub fn wait_until_starts(
    name: &str,
    start: &dyn Fn() -> std::result::Result<Start, String>,
    announce: &dyn Fn(&str),
    hold: &dyn Fn(&str) -> Result<()>,
    pause: &dyn Fn() -> Result<Ran>,
) -> Result<bool> {
    let mut waited = false;
    loop {
        crate::lock::refuse_when_stopped(&format!("waited for Chrome to start on {name}"))?;
        match start() {
            Ok(Start::Started) => {
                if waited {
                    announce(&format!("Chrome starts on {} again", place(name)));
                }
                return Ok(true);
            }
            Ok(Start::NoAddress(why)) => {
                if !waited {
                    announce(&format!(
                        "waiting for Chrome to start on {}, since a shots run draws its boards with Chrome and builds nothing there until it does; on a Mac this means the window server does not answer: {why}",
                        place(name)
                    ));
                    hold(&format!("{HOLD_STEP} {name}"))?;
                    waited = true;
                }
                if pause()? == Ran::Stopped {
                    return Ok(false);
                }
            }
            Ok(Start::Failed(why)) => fail!("Chrome cannot start on {}: {why}", place(name)),
            Err(error) => fail!("{error}"),
        }
    }
}

/// The lines for <locks>: each host that holds a build while Chrome does not
/// start there, with the build's checkout and how long it has waited.
pub fn findings(locks: &[LockHealth]) -> Vec<Advice> {
    locks
        .iter()
        .filter_map(|lock| {
            let step = lock.holder.as_ref()?.run.as_ref()?.step.as_ref()?;
            let host = step.name.strip_prefix(HOLD_STEP)?.trim();
            Some(Advice::said(format!(
                "{} cannot start Chrome, so the {} app build of {} holds checkout {} and has built nothing for {} minutes: the window server there does not answer Chrome's start, and every build with shots there stays held until it does",
                place(host),
                lock.lane.word(),
                lock.repo,
                lock.checkout,
                step.minutes
            )))
        })
        .collect()
}

#[cfg(test)]
#[path = "host_chrome_tests.rs"]
mod tests;
