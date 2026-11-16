//! The `builds cargo` runs that started on each host in the last minute.
//! cargo runs one compile job per core of the host it runs on, but a host's
//! one-minute load average shows a run only some time after it starts. So
//! when several runs start within a minute, each pick would read the loads
//! from before the others and choose the same host. `cargo_place::choose`
//! counts each host's recent runs as load its probe does not show yet
//! (`host::rank_by_room`), so a burst of runs spreads across the hosts.
//!
//! The record is the folder `cargo-starts` beside the file `hosts`: one file
//! per run, named for the pid of its `builds cargo` process, holding the host
//! it went to and when it started. A run counts while it is under a minute
//! old and its process still runs: a run that already ended adds no load. A
//! pick holds the folder's lock from reading the record until it has written
//! its own run, so picks made at the same time take turns and each sees the
//! ones before it. Files over a minute old name nothing a pick counts, and
//! the pick that reads them deletes them.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;

/// How long a run counts: the window of the load average a probe reads.
pub const WINDOW_SECONDS: i64 = 60;

/// The runs started on each host in the last minute that still run, by host
/// name.
pub type Recent = HashMap<String, u32>;

/// The record, read under its lock: the lock is held until `record`.
pub struct Starts {
    dir: PathBuf,
    _lock: File,
    pub recent: Recent,
}

fn dir_of(places: &Places) -> PathBuf {
    crate::host::hosts_file(places).with_file_name("cargo-starts")
}

/// Is the process <pid> running?
fn runs(pid: i32) -> bool {
    let signalled = unsafe { libc::kill(pid, 0) };
    signalled == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Takes the record's lock, waiting while another pick holds it, and reads
/// the runs that count at <now> (seconds since 1970).
pub fn read(places: &Places, now: i64) -> Result<Starts> {
    let dir = dir_of(places);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let lock = crate::evict::lock_exclusive_creating(&dir.join(".lock"))?;
    let mut recent = Recent::new();
    let entries = std::fs::read_dir(&dir).context(|| format!("listing {}", dir.display()))?;
    for entry in entries {
        let path = entry
            .context(|| format!("listing {}", dir.display()))?
            .path();
        let name = crate::checkout::file_name(&path);
        let Ok(pid) = name.parse::<i32>() else {
            // The lock, and a file being written (`.<pid>`).
            continue;
        };
        let (host, at) = parse(&path)?;
        if now - at >= WINDOW_SECONDS {
            std::fs::remove_file(&path).context(|| format!("removing {}", path.display()))?;
        } else if runs(pid) {
            *recent.entry(host).or_insert(0) += 1;
        }
    }
    Ok(Starts {
        dir,
        _lock: lock,
        recent,
    })
}

/// The host and the start time in the run file <path>.
fn parse(path: &Path) -> Result<(String, i64)> {
    let text = std::fs::read_to_string(path).context(|| format!("reading {}", path.display()))?;
    let Some((host, at)) = text.trim().split_once(' ') else {
        fail!("{} is not `<host> <time>`: {text:?}", path.display());
    };
    let at = at
        .parse()
        .context(|| format!("reading the time in {}", path.display()))?;
    Ok((host.to_string(), at))
}

impl Starts {
    /// Writes that the run of process <pid> started on <host> at <now>, and
    /// lets the next pick read the record.
    pub fn record(self, pid: u32, host: &str, now: i64) -> Result<()> {
        let part = self.dir.join(format!(".{pid}"));
        let file = self.dir.join(pid.to_string());
        std::fs::write(&part, format!("{host} {now}\n"))
            .context(|| format!("writing {}", part.display()))?;
        std::fs::rename(&part, &file).context(|| format!("replacing {}", file.display()))
    }
}

#[cfg(test)]
#[path = "cargo_starts_tests.rs"]
mod tests;
