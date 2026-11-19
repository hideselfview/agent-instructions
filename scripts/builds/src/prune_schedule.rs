//! Running `builds prune` on a schedule. Some of what builds leaves behind
//! only prune deletes, such as the job worktrees of retired checkouts on the
//! hosts (remote_worktrees.rs), so the dashboard daemon runs `builds prune`
//! every PRUNE_HOURS hours.
//!
//! A prune that finishes, whoever ran it, writes when it finished to
//! `<daemon dir>/pruned`. The daemon starts the next prune PRUNE_HOURS after
//! the later of that time and its own last start: a daemon that restarts
//! (each install restarts it) runs no extra prune, and the daemon tries a
//! prune that failed again PRUNE_HOURS later. While a prune runs it holds
//! `<daemon dir>/prune.lock`, and a second prune fails at once rather than
//! delete beside it. The daemon writes what its prune says to `<daemon
//! dir>/prune.log`. `builds health` says when a prune last finished, and
//! lists it under needs action when none has finished in twice PRUNE_HOURS.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::fail;
use crate::fail::{Context, Result};
use crate::health::Advice;
use crate::places::Places;
use crate::text::{local_minute, minutes_since, note};

/// What a prune says when another prune holds the prune lock.
pub const BUSY: &str = "another builds prune is running";

/// How often the daemon runs `builds prune`.
pub const PRUNE_HOURS: i64 = 6;

/// When a prune last finished, in seconds since 1970; None when no prune
/// has finished.
pub fn last_finished(places: &Places) -> Result<Option<i64>> {
    let path = places.prune_record();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => fail!("reading {}: {error}", path.display()),
    };
    match text.trim().parse() {
        Ok(at) => Ok(Some(at)),
        Err(error) => fail!(
            "{} holds {text:?}, not the time a prune finished: {error}",
            path.display()
        ),
    }
}

/// Records that a prune finished at <at>.
pub fn record_finished(places: &Places, at: i64) -> Result<()> {
    let dir = places.daemon_dir();
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    crate::whole_file::write(&places.prune_record(), format!("{at}\n"))
}

/// A running prune's hold on the prune lock, which it keeps until it ends.
pub struct Running {
    _lock: File,
}

/// Takes the prune lock for this prune without waiting; fails when another
/// prune holds it.
pub fn hold(places: &Places) -> Result<Running> {
    let dir = places.daemon_dir();
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let path = places.prune_lock();
    match crate::evict::try_lock_creating(&path)? {
        Some(lock) => Ok(Running { _lock: lock }),
        None => fail!("prune: {BUSY} and holds {}", path.display()),
    }
}

/// When the daemon starts the next prune: PRUNE_HOURS after the later of
/// <finished>, when a prune last finished, and <started>, when the daemon last
/// started one; at once (0) when neither happened.
fn next_start(finished: Option<i64>, started: Option<i64>) -> i64 {
    finished
        .max(started)
        .map_or(0, |at| at + PRUNE_HOURS * 3600)
}

/// Runs `<exe> prune`, with what it says written to the prune log, and says
/// in the daemon's log how it ended.
fn run_once(places: &Places, exe: &Path) {
    let log_path = places.prune_log();
    let ended = (|| -> std::io::Result<std::process::ExitStatus> {
        let log = File::create(&log_path)?;
        let errors = log.try_clone()?;
        // The prune is the daemon's, not the work of the agent whose session
        // started the daemon (an install restarts it from the session that
        // landed), and it takes its own use of the stores.
        Command::new(exe)
            .arg("prune")
            .env_remove(crate::ack::SESSION)
            .env_remove(crate::install_gate::VARIABLE)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(errors)
            .status()
    })();
    match ended {
        Ok(status) if status.success() => {}
        Ok(status) => note(format!(
            "the scheduled builds prune failed ({status}); it said what went wrong in {}",
            log_path.display()
        )),
        Err(error) => note(format!(
            "starting the scheduled builds prune, with its output in {}: {error}",
            log_path.display()
        )),
    }
}

/// Starts the daemon's thread that runs `<exe> prune` every PRUNE_HOURS
/// hours, the first one at once when no prune has finished in that time.
pub fn start(places: Places, exe: PathBuf) -> Result<()> {
    std::thread::Builder::new()
        .name("builds-prune".into())
        .spawn(move || {
            let mut started = None;
            loop {
                let finished = last_finished(&places).unwrap_or_else(|error| {
                    // The next prune that finishes writes the record again.
                    note(format!("{}; the daemon runs a prune now", error.text()));
                    None
                });
                let now = crate::app_timing::now();
                let next = next_start(finished, started);
                if now >= next {
                    started = Some(now);
                    run_once(&places, &exe);
                    continue;
                }
                // A prune run by hand meanwhile only moves the next start
                // later, which the record read at the scheduled time shows.
                std::thread::sleep(Duration::from_secs((next - now) as u64));
            }
        })
        .context(|| "starting a thread".to_string())?;
    Ok(())
}

/// What `builds health` says about prune at <now>: its line, and the advice
/// of a finding when no prune has finished in twice PRUNE_HOURS.
pub fn health(places: &Places, now: i64) -> (String, Option<Advice>) {
    let log = places.prune_log();
    let every = format!("the dashboard runs it every {PRUNE_HOURS}h");
    match last_finished(places) {
        Ok(Some(at)) => {
            let line = format!(
                "prune: last finished {} ({}m ago); {every}",
                local_minute(at),
                minutes_since(now, at)
            );
            let late = (now - at > 2 * PRUNE_HOURS * 3600).then(|| {
                Advice::said(format!(
                    "{line}; the dashboard's last run wrote what it said to {}",
                    log.display()
                ))
            });
            (line, late)
        }
        Ok(None) => {
            let line = format!("prune: no prune has finished; {every}");
            let advice = Advice::said(format!(
                "{line}; the dashboard writes what each run says to {}",
                log.display()
            ));
            (line, Some(advice))
        }
        Err(error) => {
            let line = format!("prune: {}", error.text());
            (line.clone(), Some(Advice::said(line)))
        }
    }
}

#[cfg(test)]
#[path = "prune_schedule_tests.rs"]
mod tests;
