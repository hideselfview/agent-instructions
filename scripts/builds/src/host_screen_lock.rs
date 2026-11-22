//! A Mac host whose screen is locked. A locked screen keeps every window of
//! a shots or UI test run from rendering, as a system alert in front of every
//! app does (host_front_app.rs), so every window step there fails. Two things
//! follow:
//!
//! - `builds health` and the dashboard name each Mac whose screen is locked
//!   under "needs action" (`findings`), from the scan's `screen_locked` line.
//! - The mac lane holds a window step for the host until its screen unlocks
//!   (`wait_until_unlocked`), and says so in the build's output, naming the
//!   host. The wait blocks on the host's own unlock notification
//!   (`WAIT_SCRIPT`). The step itself holds a display assertion on the host
//!   while it runs (app_remote.rs `keeping_display_on`), so the screen does
//!   not lock during it.

use crate::fail;
use crate::fail::Result;
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
/// the screen is unlocked, either because the host posted
/// `com.apple.screenIsUnlocked` or because the screen was already unlocked.
/// It registers for the notification before it reads the lock, so an unlock
/// between the two is not missed. A failed ioreg reads as unlocked here, and
/// the lane's next reading (`read`) fails on it.
pub const WAIT_SCRIPT: &str = r#"notifyutil -1 com.apple.screenIsUnlocked >/dev/null 2>&1 &
waiter=$!
if ioreg -n Root -d1 2>/dev/null | tr ',' '\n' | grep -Eq 'IOConsoleLocked" = Yes|CGSSessionScreenIsLocked"=Yes'; then
  wait $waiter
else
  kill $waiter 2>/dev/null
fi"#;

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

/// Blocks until <host> says its screen is unlocked (`WAIT_SCRIPT`); stops
/// early, with `Ran::Stopped`, when a takeover stops the build.
pub fn wait(ssh: &Ssh, host: &Host) -> Result<Ran> {
    crate::lock::run_in_group(
        command_at(ssh, host, WAIT_SCRIPT, Terminal::None),
        "waiting for the screen to unlock",
    )
}

/// Waits while <read> says the screen of host <name> is locked, and returns
/// true once it is not. Between readings it blocks in <wait>, which returns
/// when the host says its screen unlocked, and returns false as soon as
/// <wait> says a takeover stopped the build. The first locked reading gives
/// <announce> the words that name the host, and the end of the wait gives it
/// one line more.
pub fn wait_until_unlocked(
    name: &str,
    read: &dyn Fn() -> std::result::Result<bool, String>,
    announce: &dyn Fn(&str),
    wait: &dyn Fn() -> Result<Ran>,
) -> Result<bool> {
    let mut waited = false;
    loop {
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
                if wait()? == Ran::Stopped {
                    return Ok(false);
                }
            }
            Err(error) => fail!("{error}"),
        }
    }
}

#[cfg(test)]
#[path = "host_screen_lock_tests.rs"]
mod tests;
