//! The `builds cargo` runs this Mac placed on each host that still run.
//! cargo runs one compile job per core of the host it runs on, but a host's
//! one-minute load average shows a run only some time after it starts. So
//! when several runs start within a minute, each pick would read the loads
//! from before the others and choose the same host. `cargo_place::choose`
//! counts each host's recent runs as load its probe does not show yet
//! (`host::rank_by_room`), so runs that start close together go to different
//! hosts. The load average never shows the compiles that wait in sccache's
//! queue on a host whose cores are all busy, so `cargo_place` also counts
//! every run that still runs on each host, whatever its age, and passes over
//! a host that runs more than its share of them (`cargo_place::rank`).
//!
//! The record is the folder `cargo-starts` beside the file `hosts`: one file
//! per run, named for the pid of its `builds cargo` process, holding the host
//! it went to and when it started. A run counts as live while its process
//! runs, and as recent while it is also under a minute old. A pick holds the
//! folder's lock from reading the record until it has written its own run,
//! so builds picks a host for one run at a time and each pick sees the runs
//! picked before it. The pick that reads the file of a run whose process
//! ended deletes it.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;

/// How long a run counts: the window of the load average a probe reads.
pub const WINDOW_SECONDS: i64 = 60;

/// A number of runs on each host, by host name.
pub type Recent = HashMap<String, u32>;

/// The runs on each host that still run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Those that started in the last minute.
    pub recent: Recent,
    /// All of them.
    pub live: Recent,
}

/// The record, read under its lock. `Starts` holds the lock until `record`
/// writes the run and drops it.
pub struct Starts {
    dir: PathBuf,
    _lock: File,
    pub counts: Counts,
}

fn dir_of(places: &Places) -> PathBuf {
    crate::host::hosts_file(places).with_file_name("cargo-starts")
}

/// Takes the record's lock, waiting while another pick holds it, and reads
/// the runs that count at <now> (seconds since 1970).
pub fn read(places: &Places, now: i64) -> Result<Starts> {
    let dir = dir_of(places);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let lock = crate::flock::lock_creating(&dir.join(".lock"), libc::LOCK_EX)?;
    let mut counts = Counts::default();
    let entries = std::fs::read_dir(&dir).context(|| format!("listing {}", dir.display()))?;
    for entry in entries {
        let path = entry
            .context(|| format!("listing {}", dir.display()))?
            .path();
        let name = crate::checkout::file_name(&path);
        let Ok(pid) = name.parse::<i32>() else {
            // Skips the lock and any file that `record` is still writing
            // (`.<pid>`).
            continue;
        };
        let (host, at) = parse(&path)?;
        if !crate::procs::runs(pid) {
            std::fs::remove_file(&path).context(|| format!("removing {}", path.display()))?;
            continue;
        }
        if now - at < WINDOW_SECONDS {
            *counts.recent.entry(host.clone()).or_insert(0) += 1;
        }
        *counts.live.entry(host).or_insert(0) += 1;
    }
    Ok(Starts {
        dir,
        _lock: lock,
        counts,
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
