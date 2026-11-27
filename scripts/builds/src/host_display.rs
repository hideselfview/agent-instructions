//! Whether a Mac host can show the windows of a step, and what builds does
//! when it cannot. One script reads the host's display (`SCRIPT`): its lid and
//! displays, its screen lock and the app in front. `Reason` names each reason
//! a host cannot run a step that opens windows, the ones this reading finds
//! and the ones a step's own failure finds (Chrome, Xcode, UI automation),
//! and `Reason::rule` gives each its one rule:
//!
//! - `Rule::Wait`: the person fixes it at the Mac, so builds holds the step
//!   and waits, naming the reason: a locked screen, a system alert in front,
//!   a closed lid with no display when no host left can draw, and a Chrome
//!   that cannot start.
//! - `Rule::MoveLane`: builds records the host and runs the lane on another
//!   host: an Xcode that cannot start, and UI tests that time out turning on
//!   automation mode (host.rs `Refused`).
//!
//! Before each window step of the mac lane, the lane reads the host once
//! (`read`) and holds the step while the reading gives a reason
//! (`wait_until_ready`), this Mac included. The step itself holds a display
//! assertion on the host while it runs (app_remote.rs `keeping_display_on`),
//! so the display does not sleep during it. A build that draws window scenes
//! also reads each host before it takes a checkout, and takes a host that can
//! draw when one can (lock.rs). Every host's scan runs the same script
//! (host_scan.rs). While a host holds a step, the lane records the hold
//! beside its lock (`Wait`, holder_run.rs), and `builds health` names each
//! reason that holds a step, with how long it has waited, and each reason a
//! host's scan gives that holds no step, in one shape (`finding`).

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Fail, Result};
use crate::health::{Advice, LockHealth};
use crate::host::{Host, Ssh, Terminal, answer_value, command_at, output_at, place};
use crate::host_hold::Hold;
use crate::host_scan::Scan;
use crate::lock::Ran;

/// The apps that show macOS's system alerts: notifications that ask for an
/// answer, password prompts and the alerts of the Finder's services, such as
/// the one that asks whether to open an app downloaded from the internet.
const SYSTEM_ALERTS: [&str; 3] = [
    "UserNotificationCenter",
    "SecurityAgent",
    "CoreServicesUIAgent",
];

/// The lines a Mac runs to print what `parse` reads, each `<key>=<value>`,
/// with the programs of the system table (system.rs):
///
/// - `lid=closed` with `screens=<n>`, how many displays `system_profiler`
///   lists with a resolution, `lid=open`, or `lid=none` for a Mac with no lid
///   (`ioreg` lists no clamshell state). A Mac whose lid is closed makes no
///   virtual display for the shot renderer unless another display is
///   attached.
/// - `screen_locked=`, the lines of `ioreg`'s root entry that say whether the
///   console is locked (`"IOConsoleLocked" = Yes`) and whether the console
///   user's session has its screen locked (`"CGSSessionScreenIsLocked"=Yes`),
///   on one line.
/// - `front_app=`, lsappinfo's name and pid of the front app, on one line.
///
/// A program that fails makes its line say so, `<key>=failed: <what it
/// said>`, and `parse` fails on it; the script goes on to its next line, so
/// that the scan that runs it (host_scan.rs) reads its other lines.
pub const SCRIPT: &str = r#"if said=$(ioreg -r -k AppleClamshellState -d 1 2>&1); then
  case "$said" in
  *'"AppleClamshellState" = Yes'*)
    if displays=$(system_profiler SPDisplaysDataType 2>&1); then
      echo lid=closed
      echo "screens=$(printf '%s\n' "$displays" | grep -c 'Resolution:')"
    else
      echo "lid=failed: $(printf '%s\n' "$displays" | head -n 1)"
    fi;;
  *'"AppleClamshellState" = No'*) echo lid=open;;
  *) echo lid=none;;
  esac
else
  echo "lid=failed: $(printf '%s\n' "$said" | head -n 1)"
fi
if said=$(ioreg -n Root -d1 2>&1); then echo "screen_locked=$(printf '%s\n' "$said" | tr ',' '\n' | grep -E 'IOConsoleLocked|CGSSessionScreenIsLocked' | tr '\n' ' ')"; else echo "screen_locked=failed: $(printf '%s\n' "$said" | head -n 1)"; fi
echo "front_app=$(lsappinfo info -only name,pid "$(lsappinfo front)" 2>&1 | tr '\n' ' ')""#;

/// A Mac's lid, as `SCRIPT` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lid {
    Open,
    /// A Mac with no lid.
    None,
    /// Closed, with <screens> displays attached.
    Closed {
        screens: u32,
    },
}

/// The app that holds the focus on a Mac, as `lsappinfo` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontApp {
    /// Its name, or None when lsappinfo names none.
    pub name: Option<String>,
    pub pid: i32,
}

/// What a Mac's display reading (`SCRIPT`) says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Display {
    pub lid: Lid,
    pub locked: bool,
    pub front: FrontApp,
}

/// Why a host cannot run a step that opens windows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reason {
    /// Its screen is locked, which keeps every window from rendering.
    ScreenLocked,
    /// A system alert in front of every app keeps the focus from every window
    /// a step brings forward.
    SystemAlert { name: String, pid: i32 },
    /// Its lid is closed and it lists no other display, so macOS makes no
    /// display for the shot renderer to draw window scenes on.
    LidClosed,
    /// Chrome, which draws the boards a shots run compares, cannot start
    /// there: on a Mac, its window server does not answer (host_chrome.rs).
    /// <why> is what `builds shots chrome` said.
    ChromeCannotStart { why: String },
    /// Its Xcode cannot start, so every xcodebuild run there fails.
    XcodeCannotStart,
    /// Its UI tests time out turning on automation mode
    /// (ui_test_settings.rs).
    UiAutomationTimesOut,
}

/// What builds does about a reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// Holds the step and waits, naming the reason, while the person fixes it
    /// at the Mac.
    Wait,
    /// Records the host and runs the lane on another host.
    MoveLane,
}

impl Reason {
    /// The rule builds applies to the reason.
    pub fn rule(&self) -> Rule {
        match self {
            Reason::ScreenLocked
            | Reason::SystemAlert { .. }
            | Reason::LidClosed
            | Reason::ChromeCannotStart { .. } => Rule::Wait,
            Reason::XcodeCannotStart | Reason::UiAutomationTimesOut => Rule::MoveLane,
        }
    }

    /// The words that name the reason, after the host's name: "its screen is
    /// locked".
    pub fn words(&self) -> String {
        match self {
            Reason::ScreenLocked => "its screen is locked".to_string(),
            Reason::SystemAlert { name, pid } => {
                format!("the system alert {name} (pid {pid}) is in front of every app")
            }
            Reason::LidClosed => "its lid is closed and it lists no other display".to_string(),
            Reason::ChromeCannotStart { why } => {
                format!("Chrome cannot start there, since the window server does not answer: {why}")
            }
            Reason::XcodeCannotStart => "its Xcode cannot start".to_string(),
            Reason::UiAutomationTimesOut => {
                "its UI tests time out turning on automation mode".to_string()
            }
        }
    }
}

impl Rule {
    /// What builds does about a reason of host <name> under this rule, as a
    /// sentence.
    fn effect(self, name: &str) -> String {
        match self {
            Rule::Wait => format!(
                "builds holds each step that needs windows on {} until it ends.",
                place(name)
            ),
            Rule::MoveLane => {
                "Mac app builds skip its app checkouts until builds is told it is fixed."
                    .to_string()
            }
        }
    }
}

/// A step that a host holds before it starts: the host, the target that
/// reaches it (None for this Mac), and the reasons it gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    pub host: String,
    pub target: Option<String>,
    pub reasons: Vec<Reason>,
}

impl Wait {
    pub fn on(host: &Host, reasons: Vec<Reason>) -> Wait {
        Wait {
            host: host.name.clone(),
            target: host.target.clone(),
            reasons,
        }
    }
}

impl Display {
    /// The reasons the host cannot run a step that opens windows. A closed
    /// lid counts only when <draws>: a step that draws window scenes needs a
    /// display, and other steps run without one.
    pub fn reasons(&self, draws: bool) -> Vec<Reason> {
        let mut reasons = Vec::new();
        if self.locked {
            reasons.push(Reason::ScreenLocked);
        }
        if let Some(name) = self
            .front
            .name
            .as_deref()
            .filter(|name| SYSTEM_ALERTS.contains(name))
        {
            reasons.push(Reason::SystemAlert {
                name: name.to_string(),
                pid: self.front.pid,
            });
        }
        if draws && self.lid == (Lid::Closed { screens: 0 }) {
            reasons.push(Reason::LidClosed);
        }
        reasons
    }
}

/// Reads the lines `SCRIPT` printed in <text>: None when it printed none of
/// them, as a host that is no Mac prints none, and an error that quotes the
/// line it cannot read.
pub fn parse(text: &str) -> std::result::Result<Option<Display>, String> {
    let lid = answer_value(text, "lid");
    let locked = answer_value(text, "screen_locked");
    let front = answer_value(text, "front_app");
    if lid.is_none() && locked.is_none() && front.is_none() {
        return Ok(None);
    }
    let missing = |key: &str| format!("the display reading has no {key} line: {text:?}");
    Ok(Some(Display {
        lid: parse_lid(
            lid.ok_or_else(|| missing("lid"))?,
            answer_value(text, "screens"),
        )?,
        locked: parse_lock(locked.ok_or_else(|| missing("screen_locked"))?)?,
        front: parse_front_app(front.ok_or_else(|| missing("front_app"))?)?,
    }))
}

/// The lid that the `lid` line <line> and the `screens` line <screens> name.
fn parse_lid(line: &str, screens: Option<&str>) -> std::result::Result<Lid, String> {
    match line {
        "open" => Ok(Lid::Open),
        "none" => Ok(Lid::None),
        "closed" => match screens.and_then(|count| count.parse().ok()) {
            Some(screens) => Ok(Lid::Closed { screens }),
            None => Err(format!(
                "the display reading has no screens count for a closed lid: {screens:?}"
            )),
        },
        other => Err(format!("ioreg named no lid state: {other:?}")),
    }
}

/// Whether the `screen_locked` line <line> says the screen is locked; an
/// error that quotes the line when it holds no `IOConsoleLocked` value.
fn parse_lock(line: &str) -> std::result::Result<bool, String> {
    let squeezed: String = line.split_whitespace().collect();
    if squeezed.contains("\"IOConsoleLocked\"=Yes")
        || squeezed.contains("\"CGSSessionScreenIsLocked\"=Yes")
    {
        Ok(true)
    } else if squeezed.contains("\"IOConsoleLocked\"=No") {
        Ok(false)
    } else {
        Err(format!("ioreg named no console lock state: {line:?}"))
    }
}

/// The app that the `front_app` line <line> names; an error that quotes the
/// line when it names no pid.
///
/// lsappinfo prints the fields one of two ways, by macOS version: macOS 26
/// prints each as `"<key>"=<value>` (`"LSDisplayName"="bae" "pid"=93913`),
/// and macOS 27 describes the app, its name in quotes first and its pid
/// after `pid =` (`"Terminal" ASN:0x0-0x66066: (in front) ... pid = 43839`).
fn parse_front_app(line: &str) -> std::result::Result<FrontApp, String> {
    let unread = || format!("lsappinfo named no front app: {line:?}");
    let (name, pid) = match line.split_once(r#""pid"="#) {
        Some((_, after_pid)) => (
            line.split_once(r#""LSDisplayName"=""#)
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(name, _)| name),
            after_pid,
        ),
        None => (
            line.trim_start()
                .strip_prefix('"')
                .and_then(|rest| rest.split_once('"'))
                .map(|(name, _)| name),
            line.split_once("pid = ").ok_or_else(unread)?.1,
        ),
    };
    let pid = pid
        .split_whitespace()
        .next()
        .and_then(|pid| pid.parse().ok())
        .ok_or_else(unread)?;
    Ok(FrontApp {
        name: name.map(str::to_string),
        pid,
    })
}

/// One reading of a Mac's display: what it says, and what the script
/// printed, which the wait compares later readings with (`wait_script`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub display: Display,
    printed: String,
}

/// Reads <host>'s display; fails when the host does not say, or prints no
/// display reading, as a host that is no Mac does.
pub fn read(ssh: &Ssh, host: &Host) -> std::result::Result<Reading, String> {
    let what = format!("reading the display of {}", place(&host.name));
    let printed = output_at(ssh, host, SCRIPT, &what)?;
    let display = parse(&printed)
        .map_err(|why| format!("{what}: {why}"))?
        .ok_or_else(|| format!("{what}: it printed no display reading: {printed:?}"))?;
    Ok(Reading { display, printed })
}

/// The exit status of a waiter that a signal ended: 128 and SIGTERM.
const WAITER_KILLED: i32 = 143;

/// How often the wait script reads the display.
const POLL_SECONDS: u32 = 15;

/// The script the lane runs on a Mac whose display gives a reason: it reads
/// the display (`SCRIPT`) every <poll_seconds> and returns once the reading
/// differs from <seen>, the lane's last reading, and the lane reads the
/// display again. The host
/// sends a process started over ssh no event for a change of its display
/// (the `com.apple.screenIsUnlocked` notification did not reach waiters
/// there), so the reading is how the script learns the state. The script
/// leaves no process behind: a signal to it kills its sleep and it exits 143,
/// and it returns once its parent is gone, as when its ssh connection ends.
pub fn wait_script(poll_seconds: u32, seen: &Reading) -> String {
    // The shell's `$(...)` drops the reading's last newlines.
    let seen = crate::host::sh_quote(seen.printed.trim_end_matches('\n'));
    format!(
        r#"parent=$PPID
sleeper=
trap 'kill $sleeper 2>/dev/null' EXIT
trap 'exit 143' HUP INT TERM
reading() {{
{SCRIPT}
}}
while [ "$(reading)" = {seen} ]; do
  kill -0 $parent 2>/dev/null || exit 0
  sleep {poll_seconds} &
  sleeper=$!
  wait $sleeper
done"#
    )
}

/// Blocks until <host>'s display reading differs from <seen>
/// (`wait_script`); stops early, with `Ran::Stopped`, when a takeover stops
/// the build.
pub fn wait(ssh: &Ssh, host: &Host, seen: &Reading) -> Result<Ran> {
    crate::lock::run_in_group(
        command_at(ssh, host, &wait_script(POLL_SECONDS, seen), Terminal::None)?,
        "waiting for the display to change",
    )
}

/// Holds a step while <read> gives <host> a reason it cannot show the
/// step's windows, and returns true once it gives none. Between readings it
/// blocks in <wait>, which returns when the host's display changed or a
/// signal ended the waiter (the next reading says what holds), and returns
/// false as soon as <wait> says a takeover stopped the build. Each reading
/// whose reasons differ from the last gives <announce> the words that name
/// the host and the reasons, and <record> the hold with the time it began;
/// the end of the hold gives <announce> one line more and
/// <record> None.
pub fn wait_until_ready(
    host: &Host,
    read: &dyn Fn() -> std::result::Result<Vec<Reason>, String>,
    announce: &dyn Fn(&str),
    record: &dyn Fn(Option<Hold<Wait>>) -> Result<()>,
    wait: &dyn Fn() -> Result<Ran>,
) -> Result<bool> {
    let name = &host.name;
    crate::host_hold::hold_until_ready(
        &format!("{}'s display", place(name)),
        &|| match read() {
            Ok(reasons) if reasons.is_empty() => Ok(None),
            Ok(reasons) => Ok(Some(reasons)),
            Err(error) => fail!("{error}"),
        },
        &|reasons| {
            format!(
                "waiting for {} to show windows, since {}; fix it on that Mac",
                place(name),
                reasons
                    .iter()
                    .map(Reason::words)
                    .collect::<Vec<_>>()
                    .join(", and ")
            )
        },
        announce,
        &|holds| {
            record(holds.map(|hold| Hold {
                what: Wait::on(host, hold.what.clone()),
                since: hold.since,
            }))
        },
        &format!("{} shows windows again", place(name)),
        &|| match wait() {
            Err(Fail::Exit(WAITER_KILLED)) => {
                announce(&format!(
                    "the wait for {}'s display was killed (exit {WAITER_KILLED}); reading the display again",
                    place(name)
                ));
                Ok(Ran::Finished)
            }
            ended => ended,
        },
    )
}

/// The finding for host <name> that <reason> keeps from running steps that
/// open windows, in the one shape every such finding has: the reason with
/// <detail>, what builds does about it (`Rule`), then <fix>, the person's
/// step, and the command <run> when the fix has one.
pub fn finding(
    name: &str,
    reason: &Reason,
    detail: &str,
    fix: &str,
    run: Option<String>,
) -> Advice {
    let text = format!(
        "{} cannot run steps that open windows: {}{detail}. {} {fix}",
        place(name),
        reason.words(),
        reason.rule().effect(name)
    );
    match run {
        Some(command) => Advice::run(text, command),
        None => Advice::said(text),
    }
}

/// What the person does on host <name>, which <target> reaches, about
/// <reason>, and the command that does it when one does. The lane moves away
/// from a host whose Xcode or UI automation fails, so no hold names those
/// reasons and they have their own findings (host.rs, ui_test_settings.rs).
fn fix(name: &str, target: Option<&str>, reason: &Reason) -> (String, Option<String>) {
    let at = place(name);
    match reason {
        Reason::ScreenLocked => (format!("Unlock the screen on {at}."), None),
        Reason::SystemAlert { pid, .. } => (
            format!("Answer or close the alert on {at}, or end it with"),
            Some(crate::host::person_command(target, &format!("kill {pid}"))),
        ),
        Reason::LidClosed => (
            format!("Open the lid of {at}, or attach a display to it."),
            None,
        ),
        Reason::ChromeCannotStart { .. } => (
            format!("The build goes on once the window server on {at} answers."),
            None,
        ),
        Reason::XcodeCannotStart | Reason::UiAutomationTimesOut => (String::new(), None),
    }
}

/// The lines for the display of each host: each reason that holds a step of
/// a build that holds one of <locks>, with the build, its step and how long
/// the step has waited; then each reason a host's reading in <scans> gives a
/// step that opens no window scene that no held step names, and each host
/// whose display health could not read. A host whose scan failed, and one
/// that is no Mac, get no line from <scans>.
pub fn findings(
    scans: &[(Host, std::result::Result<Scan, String>)],
    locks: &[LockHealth],
) -> Vec<Advice> {
    let mut found = Vec::new();
    let mut held: Vec<(&str, &Reason)> = Vec::new();
    for lock in locks {
        let Some(step) = lock
            .holder
            .as_ref()
            .and_then(|holder| holder.run.as_ref())
            .and_then(|run| run.step.as_ref())
        else {
            continue;
        };
        let Some(wait) = &step.waits else { continue };
        for reason in &wait.reasons {
            let (fix, run) = fix(&wait.host, wait.target.as_deref(), reason);
            found.push(finding(
                &wait.host,
                reason,
                &format!(
                    "; the {} app build of {} has waited {} minutes on checkout {} to run step {}",
                    lock.lane.word(),
                    lock.repo,
                    step.minutes,
                    lock.checkout,
                    step.name
                ),
                &fix,
                run,
            ));
            held.push((&wait.host, reason));
        }
    }
    for (host, scan) in scans {
        let Ok(scan) = scan else { continue };
        let name = &host.name;
        let display = match &scan.display {
            Ok(Some(display)) => display,
            Ok(None) => continue,
            Err(error) => {
                found.push(Advice::said(format!(
                    "health could not read the display of {}: {error}",
                    place(name)
                )));
                continue;
            }
        };
        for reason in display.reasons(false) {
            if held.contains(&(name.as_str(), &reason)) {
                continue;
            }
            let (fix, run) = fix(name, host.target.as_deref(), &reason);
            found.push(finding(name, &reason, "", &fix, run));
        }
    }
    found
}

#[cfg(test)]
#[path = "host_display_tests.rs"]
mod tests;
