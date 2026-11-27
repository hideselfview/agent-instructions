//! The processes builds starts to outlive their caller, and how builds stops
//! a process group.
//!
//! builds detaches a process only with `Detached::start`, which starts it in a
//! session of its own and records it under the build volume,
//! `<volume>/.detached/<pid>-<started>.json`, before it returns. Health reads
//! the records to tell these processes from orphans (orphans.rs) and leaves
//! each one, with what runs under it, alone, when it stops orphans and when it
//! stops junk shells (shells.rs). The scan of each host removes a record whose
//! process has ended (host_scan.rs).

use std::collections::BTreeSet;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::procs::Processes;

/// The folder of the records, in the build volume's root. A name that starts
/// with a dot is one of the volume's own (`Places::is_reserved_name`).
pub const FOLDER: &str = ".detached";

/// How long a process group that got SIGTERM has to end before SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(10);

/// How much later than its true start a reading may put a process's start: a
/// scan subtracts a process's age, in whole seconds, from the time it read
/// before it listed the processes, and Linux gives a start in clock ticks
/// since a boot time in whole seconds.
const START_SLACK_SECONDS: i64 = 1;

/// The record of a process builds detached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detached {
    pub pid: i32,
    /// When it had started, in seconds since 1970 by its machine's clock:
    /// builds reads the clock once the start returns.
    pub started: i64,
    /// What it is to do.
    pub purpose: String,
    /// The builds process that started it.
    pub caller: Caller,
}

/// A builds process that detached another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    pub pid: i32,
    /// Its command line.
    pub command: String,
}

impl Detached {
    /// Starts <command> in a new session, so it runs on when this process,
    /// its process group or its terminal ends, and records it under the build
    /// volume <volume>, which must exist, with its <purpose>. When the record
    /// cannot be written, kills what started and fails. <command> must not
    /// ask for a process group of its own: a new session makes one.
    pub fn start(volume: &Path, command: &mut Command, purpose: &str) -> Result<Child> {
        let folder = volume.join(FOLDER);
        match std::fs::create_dir(&folder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => fail!("creating {}: {error}", folder.display()),
        }
        // SAFETY: setsid is async-signal-safe and touches no memory.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let program = command.get_program().to_string_lossy().into_owned();
        let mut child = command
            .spawn()
            .context(|| format!("starting {program} to {purpose}"))?;
        let record = Detached {
            pid: child.id() as i32,
            started: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("the clock is after 1970")
                .as_secs() as i64,
            purpose: purpose.to_string(),
            caller: Caller {
                pid: std::process::id() as i32,
                command: std::env::args_os()
                    .map(|word| word.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" "),
            },
        };
        let written = crate::whole_file::write(
            &folder.join(record.file_name()),
            serde_json::to_vec(&record).expect("a record serializes"),
        );
        if let Err(error) = written {
            // Health would take a process with no record for an orphan.
            let stopped = stop_group(record.pid, Duration::ZERO).and_then(|()| {
                child
                    .wait()
                    .map(drop)
                    .context(|| format!("waiting for {program}, which builds stopped"))
            });
            return fail::with_cleanup(Err(error), stopped);
        }
        Ok(child)
    }

    /// The name of the record's file.
    pub fn file_name(&self) -> String {
        format!("{}-{}.json", self.pid, self.started)
    }

    /// Whether the record's process is the process <pid> that started at
    /// <started>. A process that took the pid after the recorded one ended
    /// started later than the record's time.
    pub fn names(&self, pid: i32, started: i64) -> bool {
        pid == self.pid && started <= self.started + START_SLACK_SECONDS
    }
}

/// Every record under the build volume <volume>.
fn records(volume: &Path) -> Result<Vec<Detached>> {
    let folder = volume.join(FOLDER);
    let listing = match std::fs::read_dir(&folder) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", folder.display()),
    };
    let mut found = Vec::new();
    for entry in listing {
        let path = entry
            .context(|| format!("listing {}", folder.display()))?
            .path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            // A scan removed it since the listing: its process ended.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => fail!("reading {}: {error}", path.display()),
        };
        found.push(
            serde_json::from_slice(&bytes).context(|| format!("reading {}", path.display()))?,
        );
    }
    Ok(found)
}

/// The pids of the processes among <processes> that a record under <volume>
/// names. <processes> must be read before this call, so that the record of
/// each one that builds detached is written by now.
pub fn running(volume: &Path, processes: &Processes) -> Result<BTreeSet<i32>> {
    Ok(records(volume)?
        .iter()
        .filter(|record| {
            processes
                .get(record.pid)
                .is_some_and(|process| record.names(process.pid, process.started))
        })
        .map(|record| record.pid)
        .collect())
}

/// Stops every process of the group <group>: SIGTERM, then SIGKILL to what
/// still runs once <grace> has passed. Returns once the group is empty or has
/// had SIGKILL.
pub fn stop_group(group: i32, grace: Duration) -> Result<()> {
    if !signal_group(group, libc::SIGTERM)? {
        return Ok(());
    }
    let until = Instant::now() + grace;
    while signal_group(group, 0)? {
        if Instant::now() >= until {
            signal_group(group, libc::SIGKILL)?;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// Sends <signal> to the process group <group>; false when the group has no
/// process left to signal. macOS answers EPERM, not ESRCH, for a group whose
/// processes have all ended and wait to be reaped.
fn signal_group(group: i32, signal: libc::c_int) -> Result<bool> {
    // SAFETY: kill(2) touches no memory.
    if unsafe { libc::kill(-group, signal) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH | libc::EPERM) => Ok(false),
        _ => Err(Fail::Message(format!(
            "signalling process group {group}: {error}"
        ))),
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
