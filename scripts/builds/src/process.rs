//! The processes builds starts to outlive their caller, and how builds stops
//! a process group.
//!
//! builds detaches a process only with `Detached::start`, which starts it in a
//! session of its own and records it under the build volume,
//! `<volume>/.detached/<pid>-<started>.json`, before its program runs. Health
//! reads the records to tell these processes from orphans (orphans.rs) and
//! leaves each one, with what runs under it, alone, when it stops orphans and
//! when it stops junk shells (shells.rs). The scan of each host
//! (host_scan.rs) reads that host's records with `listing_script` and removes,
//! with `remove`, each one whose process has ended.

use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::host::{Host, Ssh};
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
    /// builds reads the clock once the process says its pid.
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
    /// volume <volume>, which must exist, with its <purpose>. The new process
    /// runs its program only once the record is written: when builds cannot
    /// write it, or ends first, the program never runs, so it leaves nothing
    /// half done. <command> must not ask for a process group of its own: a
    /// new session makes one.
    pub fn start(volume: &Path, command: &mut Command, purpose: &str) -> Result<Child> {
        let folder = volume.join(FOLDER);
        match std::fs::create_dir(&folder) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => fail!("creating {}: {error}", folder.display()),
        }
        let program = command.get_program().to_string_lossy().into_owned();
        let starting = || format!("starting {program} to {purpose}");
        let (recorder, announcer) = UnixStream::pair().context(starting)?;
        let (recorder_fd, announcer_fd) = (recorder.as_raw_fd(), announcer.as_raw_fd());
        // SAFETY: setsid, close, getpid, write and read are
        // async-signal-safe, and both descriptors are open until the spawn
        // returns.
        unsafe {
            command.pre_exec(move || {
                // While the new process held a copy of the recorder's end,
                // its read would not end when builds does.
                if libc::setsid() < 0 || libc::close(recorder_fd) < 0 {
                    return Err(io::Error::last_os_error());
                }
                announce_pid(announcer_fd)
            });
        }
        // The spawn returns once the program runs, so another thread records
        // the process meanwhile.
        let (spawned, recorded) = std::thread::scope(|scope| {
            let recording = std::thread::Builder::new()
                .spawn_scoped(scope, || record_announced(recorder, &folder, purpose))
                .context(starting)?;
            let spawned = command.spawn();
            // With no process left to send a pid, the recorder's read ends.
            drop(announcer);
            let recorded = recording.join().expect("the recorder does not panic");
            Ok::<_, Fail>((spawned, recorded))
        })?;
        // A record that failed is why the spawn did.
        recorded?;
        spawned.context(starting)
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

/// Sends this process's pid on <socket> and waits for a byte back, which
/// lets it go on to run its program; the end of the stream fails it. A new
/// process calls this before its exec (`pre_exec`), so it makes only
/// async-signal-safe calls.
pub fn announce_pid(socket: RawFd) -> io::Result<()> {
    // SAFETY: the caller keeps the socket open; this value never closes it,
    // and the new process's copy closes on exec.
    let stream = std::mem::ManuallyDrop::new(unsafe { UnixStream::from_raw_fd(socket) });
    let mut stream = &*stream;
    stream.write_all(&unsafe { libc::getpid() }.to_ne_bytes())?;
    stream.read_exact(&mut [0])
}

/// Reads the pid that a process `Detached::start` spawns sends on <stream>,
/// writes its record in <folder>, then lets it go on. When the record cannot
/// be written, dropping <stream> stops the process before its program runs.
fn record_announced(mut stream: UnixStream, folder: &Path, purpose: &str) -> Result<()> {
    let told = || format!("hearing from the process that is to {purpose}");
    let mut pid = [0; size_of::<libc::pid_t>()];
    match stream.read_exact(&mut pid) {
        Ok(()) => {}
        // The start failed before there was a process, as the spawn says.
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
        Err(error) => fail!("{}: {error}", told()),
    }
    let record = Detached {
        pid: libc::pid_t::from_ne_bytes(pid),
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
    crate::whole_file::write(
        &folder.join(record.file_name()),
        serde_json::to_vec(&record).expect("a record serializes"),
    )?;
    stream.write_all(&[0]).context(told)
}

/// The start of each line `listing_script` prints.
const LISTED: &str = "detached=";

/// The folder of <host>'s records, as a word of a script there: under
/// <volume>, this Mac's build volume, or on another host under the build
/// volume that `remote::root_script` names.
fn folder_word(host: &Host, volume: &Path) -> String {
    match host.target {
        Some(_) => format!("\"$BUILDS_ROOT\"/{FOLDER}"),
        None => crate::host::sh_quote(&volume.join(FOLDER).to_string_lossy()),
    }
}

/// The shell lines that print each of <host>'s records on a line of its own,
/// which `listed` reads; they follow `remote::root_script`. A script runs
/// them before it lists the processes, so a record they print whose process
/// the listing lacks is of a process that ended. A record that another
/// reading removes meanwhile is of one that ended too. <volume> is this Mac's
/// build volume.
pub fn listing_script(host: &Host, volume: &Path) -> String {
    format!(
        r#"for record in {folder}/*.json; do
  [ -e "$record" ] || continue
  if text=$(cat "$record"); then
    printf '{LISTED}%s\n' "$text"
  elif [ -e "$record" ]; then
    exit 1
  fi
done"#,
        folder = folder_word(host, volume),
    )
}

/// The records among the lines of <text>, what a script that ran
/// `listing_script` printed.
pub fn listed(text: &str) -> std::result::Result<Vec<Detached>, String> {
    text.lines()
        .filter_map(|line| line.strip_prefix(LISTED))
        .map(|record| {
            serde_json::from_str(record).map_err(|error| {
                format!(
                    "builds cannot read the host's record of a detached process ({error}): {record}"
                )
            })
        })
        .collect()
}

/// Removes the records <ended> from <host>. <volume> is this Mac's build
/// volume.
pub fn remove(
    ssh: &Ssh,
    host: &Host,
    volume: &Path,
    ended: &[Detached],
) -> std::result::Result<(), String> {
    let folder = folder_word(host, volume);
    let files: Vec<String> = ended
        .iter()
        .map(|record| format!("{folder}/{}", record.file_name()))
        .collect();
    crate::host::output_at(
        ssh,
        host,
        &format!(
            "{}rm -f {}",
            crate::remote::root_script(host),
            files.join(" ")
        ),
        &format!(
            "removing the records of ended processes on {}",
            crate::host::place(&host.name)
        ),
    )
    .map(drop)
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
