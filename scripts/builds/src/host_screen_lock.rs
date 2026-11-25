//! A Mac host whose screen is locked. A locked screen keeps every window of
//! a shots or UI test run from rendering, as a system alert in front of every
//! app does (host_front_app.rs), so every window step there fails. Two things
//! follow:
//!
//! - `builds health` and the dashboard name each Mac whose screen is locked
//!   under "needs action" (`findings`), from the scan's `screen_locked` line.
//! - The mac lane holds a window step for the host until its screen unlocks
//!   (`wait_until_unlocked`), and says so in the build's output, naming the
//!   host. The wait runs a script on the host (`wait_script`) that reads the
//!   lock every 15 seconds and listens for the host's unlock notification,
//!   which does not always reach it. The step itself holds a display
//!   assertion on the host while it runs (app_remote.rs `keeping_display_on`),
//!   so the screen does not lock during it.

use crate::fail;
use crate::fail::{Fail, Result};
use crate::health::Advice;
use crate::host::{Host, Ssh, Terminal, command_at, output_of, place};
use crate::host_scan::Scan;
use crate::lock::Ran;

/// The line the scan script runs on a Mac (host_scan.rs) to print what
/// `parse` reads: the lines of `ioreg`'s root entry that say whether the
/// console is locked (`"IOConsoleLocked" = Yes`) and whether the console
/// user's session has its screen locked (`"CGSSessionScreenIsLocked"=Yes`),
/// on one line. When ioreg fails, the line says so, and `parse` fails on it.
pub const SCRIPT_LINE: &str = r#"if said=$(ioreg -n Root -d1 2>&1); then echo "screen_locked=$(printf '%s\n' "$said" | tr ',' '\n' | grep -E 'IOConsoleLocked|CGSSessionScreenIsLocked' | tr '\n' ' ')"; else echo "screen_locked=ioreg failed: $(printf '%s\n' "$said" | head -n 1)"; fi"#;

/// The script the lane runs on a Mac whose screen is locked: it returns once
/// the screen reads unlocked. It reads the lock every <poll_seconds>, and
/// listens for `com.apple.screenIsUnlocked`, which it registers for before it
/// reads the lock, so an unlock between the two is not missed; the listener
/// does not always get the notification, so the reading is what ends the wait
/// when it does not. When the listener ends, the script ends with its exit
/// status (0 after the notification, 143 when something killed it, another
/// status when it failed). A failed ioreg reads as unlocked here, and the
/// lane's next reading (`read`) fails on it. The script leaves no process
/// behind: a signal to it kills the listener and the sleep, and it returns
/// once its parent is gone, as when its ssh connection ends.
pub fn wait_script(poll_seconds: u32) -> String {
    format!(
        r#"parent=$PPID
notifyutil -1 com.apple.screenIsUnlocked >/dev/null 2>&1 &
waiter=$!
sleeper=
trap 'kill $waiter $sleeper 2>/dev/null' EXIT
trap 'exit 143' HUP INT TERM
while ioreg -n Root -d1 2>/dev/null | tr ',' '\n' | grep -Eq 'IOConsoleLocked" = Yes|CGSSessionScreenIsLocked"=Yes'; do
  kill -0 $parent 2>/dev/null || exit 0
  kill -0 $waiter 2>/dev/null || {{ wait $waiter; exit $?; }}
  sleep {poll_seconds} &
  sleeper=$!
  wait $sleeper
done"#
    )
}

/// The exit status of a waiter that a signal ended: 128 and SIGTERM.
const WAITER_KILLED: i32 = 143;

/// How often the wait script reads the lock.
const POLL_SECONDS: u32 = 15;

/// Reads the scan's `screen_locked` line, <line>: None when the host printed
/// none, as a host that runs no ioreg does, Some whether the screen is
/// locked, and an error that quotes the line when it holds no
/// `IOConsoleLocked` value.
pub fn parse(line: Option<&str>) -> std::result::Result<Option<bool>, String> {
    let Some(line) = line else {
        return Ok(None);
    };
    let squeezed: String = line.split_whitespace().collect();
    if squeezed.contains("\"IOConsoleLocked\"=Yes")
        || squeezed.contains("\"CGSSessionScreenIsLocked\"=Yes")
    {
        Ok(Some(true))
    } else if squeezed.contains("\"IOConsoleLocked\"=No") {
        Ok(Some(false))
    } else {
        Err(format!("ioreg named no console lock state: {line:?}"))
    }
}

/// The lines for <scans>: each host whose screen is locked, and each host
/// whose lock health could not read. A host whose scan failed gets no line
/// here.
pub fn findings(scans: &[(Host, std::result::Result<Scan, String>)]) -> Vec<Advice> {
    scans
        .iter()
        .filter_map(|(host, scan)| {
            let scan = scan.as_ref().ok()?;
            let place = place(&host.name);
            match &scan.screen_locked {
                Ok(Some(true)) => Some(Advice::said(format!(
                    "{place} has its screen locked, so every Mac window shot there fails, and the mac lane holds window steps for {place} until it unlocks; unlock the screen on {place}"
                ))),
                Ok(_) => None,
                Err(error) => Some(Advice::said(format!(
                    "health could not tell whether the screen is locked on {place}: {error}"
                ))),
            }
        })
        .collect()
}

/// Asks <host> whether its screen is locked; fails when the host does not
/// say.
pub fn read(ssh: &Ssh, host: &Host) -> std::result::Result<bool, String> {
    let said = output_of(
        command_at(ssh, host, &format!("{SCRIPT_LINE}\n"), Terminal::None),
        &format!(
            "reading whether {} has its screen locked",
            place(&host.name)
        ),
    )?;
    parse(crate::host::answer_value(&said, "screen_locked"))?
        .ok_or_else(|| format!("{} printed no lock state: {said:?}", place(&host.name)))
}

/// Blocks until <host> says its screen is unlocked (`wait_script`); stops
/// early, with `Ran::Stopped`, when a takeover stops the build.
pub fn wait(ssh: &Ssh, host: &Host) -> Result<Ran> {
    crate::lock::run_in_group(
        command_at(ssh, host, &wait_script(POLL_SECONDS), Terminal::None),
        "waiting for the screen to unlock",
    )
}

/// Waits while <read> says the screen of host <name> is locked, and returns
/// true once it is not. Between readings it blocks in <wait>, which returns
/// when the host says its screen unlocked or a signal ended the waiter (the next
/// reading says whether the screen is unlocked), and returns false as soon as
/// <wait> says a takeover stopped the build. The first locked reading gives
/// <announce> the words that name the host, and the end of the wait gives it
/// one line more.
pub fn wait_until_unlocked(
    name: &str,
    read: &dyn Fn() -> std::result::Result<bool, String>,
    announce: &dyn Fn(&str),
    wait: &dyn Fn() -> Result<Ran>,
) -> Result<bool> {
    crate::host_hold::hold_until_ready(
        &format!("{name}'s screen to unlock"),
        &|| match read() {
            Ok(false) => Ok(None),
            Ok(true) => Ok(Some(format!(
                "waiting for {}'s screen to unlock, since a locked screen keeps every window shot there from rendering; unlock it on that Mac",
                place(name)
            ))),
            Err(error) => fail!("{error}"),
        },
        announce,
        &|| Ok(()),
        &format!("{}'s screen is unlocked", place(name)),
        &|| match wait() {
            Err(Fail::Exit(WAITER_KILLED)) => Ok(Ran::Finished),
            ended => ended,
        },
    )
}

#[cfg(test)]
#[path = "host_screen_lock_tests.rs"]
mod tests;
