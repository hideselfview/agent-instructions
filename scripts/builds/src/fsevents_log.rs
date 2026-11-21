//! Keeping macOS from storing a history of the build volume's file events.
//! fseventsd writes each file change on a volume to a history that programs
//! replay (`FSEventStreamCreate` with an older event id), which costs it work
//! on every write while agents build. A file named `no_log` in the volume's
//! `.fseventsd` folder stops the history; macOS reads it when it mounts the
//! volume. The file does not stop live events: a program that follows a folder
//! of the volume still gets them, and fseventsd works for each such program.
//! Nothing in builds follows the volume (sizes.rs records the sizes that
//! commands measure instead). Only root reads or writes `.fseventsd`, so
//! builds asks FSEvents for the UUID of the volume's history, which it gives a
//! volume that has one and not one that has none (the build volume answers
//! none on this Mac while the data volume answers one).
//!
//! `builds host setup` creates the file through sudo, and `builds health`
//! lists a build volume that keeps a history.

use crate::fail::Result;
use crate::host::{self, Host, sh_quote};
use crate::places::Places;

/// Does fseventsd keep a history of the build volume's events? macOS gives a
/// volume with a `no_log` file none, and FSEvents has no UUID for its device.
/// False off macOS, which has no FSEvents.
pub fn records_events(places: &Places) -> Result<bool> {
    use crate::fail::Context;
    use std::os::unix::fs::MetadataExt;
    let meta =
        std::fs::metadata(&places.root).context(|| format!("reading {}", places.root.display()))?;
    Ok(crate::daemon::fsevents::device_keeps_events(meta.dev()))
}

/// The command that stops the volume's history of events.
fn touch_command(places: &Places) -> String {
    format!(
        "sudo touch {}",
        sh_quote(&places.root.join(".fseventsd/no_log").display().to_string())
    )
}

/// The commands that remount the volume, so that macOS reads the file: stop
/// everything that uses the volume first.
fn remount_commands(places: &Places) -> Vec<String> {
    let root = sh_quote(&places.root.display().to_string());
    vec![
        format!("diskutil unmount {root}"),
        format!("diskutil mount \"$(df -P {root} | awk 'NR==2 {{print $1}}')\""),
    ]
}

/// The finding `builds health` makes of <recorded>, what `records_events`
/// found for the build volume: a line while macOS keeps a history of its
/// events, none while it keeps none.
pub fn finding_of(places: &Places, recorded: Result<bool>) -> Option<crate::health::Finding> {
    let text = match recorded {
        Ok(false) => return None,
        Ok(true) => format!(
            "fsevents: macOS keeps a history of file events for the build volume {}, which fseventsd writes on every file change while agents build. In a terminal, run `{}`, stop the dashboard and every build, run `{}`, then run builds health again",
            places.root.display(),
            touch_command(places),
            remount_commands(places).join("` and `")
        ),
        Err(error) => format!(
            "fsevents: builds could not check whether macOS keeps a history of file events for the build volume: {}",
            error.text()
        ),
    };
    Some(crate::health::Finding::new(
        crate::health::FindingKind::EventLog,
        crate::health::Advice::said(text),
    ))
}

/// The finding `builds health` adds for this Mac's build volume.
pub fn finding(places: &Places) -> Option<crate::health::Finding> {
    if !cfg!(target_os = "macos") || !places.root.is_dir() {
        return None;
    }
    finding_of(places, records_events(places))
}

/// `builds host setup`'s step for the build volume's events: when fseventsd
/// keeps a history of them, creates the `no_log` file through sudo (`host::sudo_step`),
/// which fails with no one at this terminal to type the password. The volume
/// keeps its history until it is remounted, which this step does not do, since a
/// volume in use refuses to unmount.
pub fn set_up(places: &Places, ssh: &host::Ssh, host: &Host, at_terminal: bool) -> Result<String> {
    if host.target.is_some() {
        return Ok("builds checks the file events of this Mac's build volume only".to_string());
    }
    if !records_events(places)? {
        return Ok("fseventsd keeps no history of file events for its build volume".to_string());
    }
    host::sudo_step(
        &host.name,
        at_terminal,
        &format!(
            "macOS keeps a history of file events for the build volume {}",
            places.root.display()
        ),
        &[touch_command(places)],
        &mut |script| host::run_at_terminal(ssh, host, script),
    )?;
    Ok(format!(
        "created the no_log file in the build volume's .fseventsd folder; stop the dashboard and every build, then run {} to make macOS read it",
        host::shown_commands(&remount_commands(places))
    ))
}

#[cfg(test)]
#[path = "fsevents_log_tests.rs"]
mod tests;
