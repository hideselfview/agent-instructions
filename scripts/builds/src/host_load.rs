//! Whether the build hosts' cores do the work that waits. While items are
//! ready to dispatch, the coordinator keeps every host's cores busy, and while
//! a host's load is well over its cores, it starts nothing new. `builds health`
//! lists a host whose cores sit idle while items are ready, and a host over its
//! cores, under "needs action".
//!
//! Both judge the 5-minute load average: health runs every 5 minutes, and the
//! 1-minute average rises and falls with each build's steps.

use crate::health::Advice;
use crate::host::{Host, Probe};
use crate::lists::store::Loaded;

/// Cores sit idle on a host whose 5-minute load is under this share of its
/// cores.
const IDLE_SHARE: f64 = 0.75;
/// A host whose 5-minute load is over this share of its cores is over them.
const OVER_SHARE: f64 = 1.25;

/// The lines for <hosts>, each with its probe, while <ready> items are ready
/// to dispatch: each host that did not answer, each host over its cores, and
/// each host with idle cores while items are ready that `builds cargo` can
/// give work to. `host::rank_by_room` skips a host that does not answer,
/// refuses jobs, is under <floor_kib> of free disk or has no measured core
/// speed, so agents started for its idle cores would build elsewhere.
pub fn findings(
    hosts: &[(Host, std::result::Result<Probe, String>)],
    ready: usize,
    floor_kib: u64,
) -> Vec<Advice> {
    let mut found = Vec::new();
    for (host, probe) in hosts {
        let answer = match probe {
            Ok(answer) => answer,
            Err(error) => {
                found.push(Advice::said(format!(
                    "{} did not say its load, so health cannot tell whether its cores are idle or over: {error}",
                    crate::host::place(&host.name)
                )));
                continue;
            }
        };
        let five = answer.load[1];
        let cores = f64::from(answer.cores);
        let load = format!("its 5-minute load is {five:.1} on {} cores", answer.cores);
        if five > OVER_SHARE * cores {
            found.push(Advice::said(format!(
                "{} is over its cores: {load}; start nothing new",
                crate::host::place(&host.name)
            )));
        } else if ready > 0
            && five < IDLE_SHARE * cores
            && host.speed.is_some()
            && crate::host::passed_over(host, probe, floor_kib).is_none()
        {
            found.push(Advice::said(format!(
                "cores idle on {} while {}: {load}; start agents",
                crate::host::place(&host.name),
                crate::text::counted(ready, "item is ready", "items are ready")
            )));
        }
    }
    found
}

/// The items ready to dispatch (`builds next`) in every project whose store
/// loads. `lists::running::findings` names each store that fails to load.
pub fn ready_items(loaded: &std::result::Result<Vec<Loaded>, String>) -> usize {
    loaded
        .iter()
        .flatten()
        .filter_map(|store| store.lists.as_ref().ok())
        .map(|lists| crate::lists::derive::dispatch_order(lists).len())
        .sum()
}

#[cfg(test)]
#[path = "host_load_tests.rs"]
mod tests;
