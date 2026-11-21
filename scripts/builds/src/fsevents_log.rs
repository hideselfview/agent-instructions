//! Keeping macOS from recording file events for the build volume. fseventsd
//! writes an event for every file change on a volume and hands them to every
//! program that follows a folder there, which cost a core while agents built.
//! A file named `no_log` in the volume's `.fseventsd` folder turns that
//! recording off; macOS reads it when it mounts the volume. Only root reads or
//! writes that folder, so builds finds out whether events flow by making a
//! change on the volume and asking FSEvents for it.
//!
//! `builds host setup` creates the file through sudo, `builds health` lists a
//! build volume that still records events, and nothing in builds follows the
//! volume (sizes.rs records the sizes that commands measure instead).

use crate::fail::Result;
use crate::host::{self, Host, sh_quote};
use crate::places::Places;

/// How long the probe waits for an event before it says the volume records
/// none.
#[cfg(target_os = "macos")]
const WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Does fseventsd record the changes made on the build volume? Makes a file
/// in a folder of the volume and asks FSEvents, from an event id taken before
/// the change, for the change; false off macOS, which has no FSEvents.
#[cfg(target_os = "macos")]
pub fn records_events(places: &Places) -> Result<bool> {
    use crate::fail::Context;
    let dir = places.daemon_dir().join("fsevents-probe");
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let dir = std::fs::canonicalize(&dir).context(|| format!("resolving {}", dir.display()))?;
    let (told, heard) = std::sync::mpsc::channel();
    let since = crate::daemon::fsevents::Stream::current_event_id();
    let stream = crate::daemon::fsevents::Stream::start_since(
        since,
        &[dir.as_path()],
        &[],
        0.0,
        Box::new(move |events| {
            let _ = told.send(events);
        }),
    );
    // The stream is live a moment after it starts; the history it replays
    // from <since> covers a change made before then.
    let file = dir.join(format!("probe-{}", std::process::id()));
    std::fs::write(&file, b"x").context(|| format!("writing {}", file.display()))?;
    let deadline = std::time::Instant::now() + WAIT;
    let mut seen = false;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match heard.recv_timeout(left) {
            Ok(events) if events.iter().any(|event| event.path.starts_with(&dir)) => {
                seen = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    drop(stream);
    std::fs::remove_file(&file).context(|| format!("removing {}", file.display()))?;
    Ok(seen)
}

#[cfg(not(target_os = "macos"))]
pub fn records_events(_places: &Places) -> Result<bool> {
    Ok(false)
}

/// The command that turns the volume's event recording off.
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

/// What `builds health` says of a build volume whose events fseventsd records,
/// or None when it records none (or this machine is not a Mac with one).
pub fn finding(places: &Places) -> Option<crate::health::Advice> {
    if !cfg!(target_os = "macos") || !places.root.is_dir() {
        return None;
    }
    match records_events(places) {
        Ok(false) => None,
        Ok(true) => Some(crate::health::Advice::said(format!(
            "fsevents: macOS records file events for the build volume {}, and fseventsd then uses a core while agents build. In a terminal, run `{}`, stop the dashboard and every build, run `{}`, then run builds health again",
            places.root.display(),
            touch_command(places),
            remount_commands(places).join("` and `")
        ))),
        Err(error) => Some(crate::health::Advice::said(format!(
            "fsevents: builds could not check whether macOS records file events for the build volume: {}",
            error.text()
        ))),
    }
}

/// `builds host setup`'s step for the build volume's events: when fseventsd
/// records them, creates the `no_log` file through sudo (`host::sudo_step`),
/// which fails with no one at this terminal to type the password. The volume
/// stays recorded until it is remounted, which this step does not do, since a
/// volume in use refuses to unmount.
pub fn set_up(places: &Places, ssh: &host::Ssh, host: &Host, at_terminal: bool) -> Result<String> {
    if host.target.is_some() {
        return Ok("builds checks the file events of this Mac's build volume only".to_string());
    }
    if !records_events(places)? {
        return Ok("fseventsd records no file events for its build volume".to_string());
    }
    host::sudo_step(
        &host.name,
        at_terminal,
        &format!(
            "macOS records file events for the build volume {}",
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
