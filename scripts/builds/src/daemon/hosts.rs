//! The hosts' rows: each host builds can run a job on (this Mac first), with
//! what it said about itself (cores, load, free disk) and what it is running
//! now, read from the `builds cargo` record in the project stores
//! (cargo_runs.rs). The daemon reads them on a clock of their own: asking a
//! host takes an ssh round trip, and a host that does not answer takes ten
//! seconds to give up.

use std::path::PathBuf;

use serde::Serialize;

use crate::cargo_runs;
use crate::db::DATABASE;
use crate::fail::{Context, Result};
use crate::host::{self, Ssh};
use crate::places::Places;

/// One host's row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HostPage {
    pub name: String,
    /// The ssh target; None for this Mac.
    pub target: Option<String>,
    pub cores: u32,
    /// Cores minus the one-minute load average; None when the host did not
    /// answer.
    pub room: Option<f64>,
    pub free_kib: Option<u64>,
    /// What went wrong asking, when the host did not answer.
    pub unreachable: Option<String>,
    /// The cargo runs on it that have not ended, oldest first.
    pub running: Vec<RunningPage>,
    /// How many cargo runs started on it in the last hour.
    pub last_hour: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunningPage {
    pub repo: String,
    pub checkout: String,
    /// cargo's subcommand and arguments.
    pub command: String,
    pub started_at: i64,
}

/// Every store's database under the lists folder.
fn databases(places: &Places) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(&places.lists) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => crate::fail!("listing {}: {error}", places.lists.display()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry
            .context(|| format!("listing {}", places.lists.display()))?
            .path()
            .join(DATABASE);
        if path.is_file() {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// Asks every host about itself and reads what each is running at <now>
/// (seconds since 1970).
pub fn read(places: &Places, ssh: &Ssh, now: i64) -> Result<Vec<HostPage>> {
    let probes = host::probe_all(places, ssh, host::all(places)?);
    let mut running = Vec::new();
    let mut started = Vec::new();
    for database in databases(places)? {
        running.extend(cargo_runs::running(&database)?);
        started.extend(cargo_runs::started_since(&database, now - 3600)?);
    }
    running.sort_by_key(|run| run.started_at);
    Ok(probes
        .into_iter()
        .map(|(host, probe)| HostPage {
            running: running
                .iter()
                .filter(|run| run.host == host.name)
                .map(|run| RunningPage {
                    repo: run.repo.clone(),
                    checkout: run.checkout.clone(),
                    command: run.command.clone(),
                    started_at: run.started_at,
                })
                .collect(),
            last_hour: started
                .iter()
                .filter(|(name, _)| *name == host.name)
                .map(|(_, count)| *count)
                .sum(),
            room: probe.as_ref().ok().map(host::Probe::room),
            free_kib: probe.as_ref().ok().map(|probe| probe.free_kib),
            unreachable: probe.as_ref().err().cloned(),
            name: host.name,
            target: host.target,
            cores: host.cores,
        })
        .collect())
}

#[cfg(test)]
#[path = "hosts_tests.rs"]
mod tests;
