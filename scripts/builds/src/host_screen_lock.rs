//! A Mac host whose screen is locked. A locked screen keeps every window of
//! a shots or UI test run from rendering, as a system alert in front of every
//! app does (host_front_app.rs), so every window step there fails. Two things
//! follow:
//!
//! - `builds health` and the dashboard name each Mac whose screen is locked
//!   under "needs action" (`findings`), from the scan's `screen_locked` line.
//! - The mac lane holds a window step for the host until its screen unlocks
//!   (`wait_until_unlocked`), and says so in the build's output, naming the
//!   host. The step itself holds a display assertion on the host while it
//!   runs (app_remote.rs `keeping_display_on`), so the screen does not lock
//!   during it.

use std::time::Duration;

use crate::fail;
use crate::fail::Result;
use crate::health::Advice;
use crate::host::{Accept, Host, Ssh, Terminal, command_at, output_of, place};
use crate::host_scan::Scan;

/// The line the scan script runs on a Mac (host_scan.rs) to print what
/// `parse` reads: the lines of `ioreg`'s root entry that say whether the
/// console is locked (`"IOConsoleLocked" = Yes`) and whether the console
/// user's session has its screen locked (`"CGSSessionScreenIsLocked"=Yes`),
/// on one line.
pub const SCRIPT_LINE: &str = r#"echo "screen_locked=$(ioreg -n Root -d1 2>&1 | tr ',' '\n' | grep -E 'IOConsoleLocked|CGSSessionScreenIsLocked' | tr '\n' ' ')""#;

/// How long the lane waits between two readings of a locked screen.
pub const POLL: Duration = Duration::from_secs(5);

/// Reads the scan's `screen_locked` line, <line>: whether the screen is
/// locked. A host that printed none, as a host that runs no ioreg does, or
/// whose lines say neither, is not locked.
pub fn parse(line: Option<&str>) -> bool {
    let Some(line) = line else {
        return false;
    };
    let squeezed: String = line.split_whitespace().collect();
    squeezed.contains("\"IOConsoleLocked\"=Yes")
        || squeezed.contains("\"CGSSessionScreenIsLocked\"=Yes")
}

/// The lines for <scans>: each host whose screen is locked. A host whose
/// scan failed gets no line here.
pub fn findings(scans: &[(Host, std::result::Result<Scan, String>)]) -> Vec<Advice> {
    scans
        .iter()
        .filter_map(|(host, scan)| {
            let scan = scan.as_ref().ok()?;
            let place = place(&host.name);
            scan.screen_locked.then(|| {
                Advice::said(format!(
                    "{place} has its screen locked, so every Mac window shot there fails, and the mac lane holds window steps for {place} until it unlocks; unlock the screen on {place}"
                ))
            })
        })
        .collect()
}

/// Asks <host> whether its screen is locked.
pub fn read(ssh: &Ssh, host: &Host) -> std::result::Result<bool, String> {
    let said = output_of(
        command_at(ssh, host, SCRIPT_LINE, Terminal::None),
        &format!(
            "reading whether {} has its screen locked",
            place(&host.name)
        ),
        Accept::Zero,
    )?;
    Ok(parse(crate::host::answer_value(&said, "screen_locked")))
}

/// Waits while <read> says the screen of host <name> is locked, reading it
/// again after each <pause>, and returns true once it is not. Returns false
/// as soon as <stopped> says a takeover stopped the build. The first locked
/// reading gives <announce> the words that name the host, and the end of the
/// wait gives it one line more.
pub fn wait_until_unlocked(
    name: &str,
    read: &dyn Fn() -> std::result::Result<bool, String>,
    announce: &dyn Fn(&str),
    stopped: &dyn Fn() -> bool,
    pause: &dyn Fn(),
) -> Result<bool> {
    let mut waited = false;
    loop {
        if stopped() {
            return Ok(false);
        }
        crate::lock::refuse_when_stopped(&format!("waited for {name}'s screen to unlock"))?;
        match read() {
            Ok(false) => {
                if waited {
                    announce(&format!("{}'s screen is unlocked", place(name)));
                }
                return Ok(true);
            }
            Ok(true) => {
                if !waited {
                    announce(&format!(
                        "waiting for {}'s screen to unlock, since a locked screen keeps every window shot there from rendering; unlock it on that Mac",
                        place(name)
                    ));
                    waited = true;
                }
                pause();
            }
            Err(error) => fail!("{error}"),
        }
    }
}

#[cfg(test)]
#[path = "host_screen_lock_tests.rs"]
mod tests;
