//! The other hosts' load: each host builds can run a job on besides this Mac,
//! with its cores and the load averages it said when asked (the dashboard
//! draws them as it draws this Mac's). The daemon asks on a clock of its own:
//! asking a host takes an ssh round trip, and a host that does not answer
//! takes ten seconds to give up.

use serde::Serialize;

use crate::fail::Result;
use crate::host::{self, Ssh};
use crate::places::Places;

/// One host's load card.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct HostPage {
    pub name: String,
    pub cores: u32,
    /// The 1, 5 and 15-minute load averages; None when the host did not
    /// answer.
    pub load: Option<[f64; 3]>,
    /// (seconds since 1970, 1-minute load) for the last ten minutes. The
    /// daemon fills it as it asks; `read` leaves it empty.
    pub history: Vec<(i64, f64)>,
    /// What went wrong asking, when the host did not answer.
    pub unreachable: Option<String>,
    /// What git printed when it failed on a host that answered: the host
    /// refuses every job.
    pub refuses: Option<String>,
}

/// Asks every host besides this Mac about itself.
pub fn read(places: &Places, ssh: &Ssh) -> Result<Vec<HostPage>> {
    let others = host::all(places)?
        .into_iter()
        .filter(|host| host.target.is_some())
        .collect();
    Ok(host::probe_all(places, ssh, others)
        .into_iter()
        .map(|(host, probe)| HostPage {
            load: probe.as_ref().ok().map(|probe| probe.load),
            history: Vec::new(),
            unreachable: probe.as_ref().err().cloned(),
            refuses: probe
                .as_ref()
                .ok()
                .and_then(|probe| probe.git_fails.clone()),
            name: host.name,
            cores: host.cores,
        })
        .collect())
}

#[cfg(test)]
#[path = "hosts_tests.rs"]
mod tests;
