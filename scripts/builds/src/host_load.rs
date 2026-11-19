//! Whether the build hosts' cores do the work that waits. While items are
//! ready to dispatch, the coordinator keeps every host's cores busy, and while
//! every host that takes cargo work is well over its cores, it starts nothing
//! new, except items of project builds. `builds health` lists a host whose
//! cores sit idle while items are ready, and a host over its cores, under
//! "needs action", and every ready item of project builds that no agent runs.
//! A host over its cores holds nothing back while another host that takes cargo
//! work has idle cores, because `builds cargo` sends the work there.
//!
//! Both judge the 5-minute load average: health runs every 5 minutes, and the
//! 1-minute average rises and falls with each build's steps.

use crate::health::Advice;
use crate::host::{Host, PassedOver, Probe};
use crate::lists::store::Loaded;

/// Cores sit idle on a host whose 5-minute load is under this share of its
/// cores.
const IDLE_SHARE: f64 = 0.75;
/// A host whose 5-minute load is over this share of its cores is over them.
const OVER_SHARE: f64 = 1.25;

/// The lines for <hosts>, each with its probe, while <ready> items are ready
/// to dispatch: each host that did not answer, each host over its cores, and
/// each host with idle cores while items are ready. A host that `builds cargo`
/// can give work to (`host::takes_cargo`, with <floor_kib> of free disk) gets
/// "start agents"; any other gets the reason `builds cargo` passes it over and
/// the fix, because agents started for its cores would build elsewhere.
pub fn findings(
    hosts: &[(Host, std::result::Result<Probe, String>)],
    ready: usize,
    floor_kib: u64,
) -> Vec<Advice> {
    let mut found = Vec::new();
    // The hosts `builds cargo` can give work to whose 5-minute load leaves
    // idle cores, each with its place: while one exists, the others' load
    // holds back nothing, since `builds cargo` sends the work there.
    let roomy: Vec<(&str, String)> = hosts
        .iter()
        .filter_map(|(host, probe)| {
            let answer = probe.as_ref().ok()?;
            let idle = answer.load[1] < IDLE_SHARE * f64::from(answer.cores);
            (idle && crate::host::takes_cargo(host, probe, floor_kib).is_ok())
                .then(|| (host.name.as_str(), crate::host::place(&host.name)))
        })
        .collect();
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
            let elsewhere: Vec<&str> = roomy
                .iter()
                .filter(|(name, _)| *name != host.name)
                .map(|(_, place)| place.as_str())
                .collect();
            let place = crate::host::place(&host.name);
            found.push(Advice::said(if elsewhere.is_empty() {
                format!(
                    "{place} is over its cores: {load}; start nothing new, except builds items, which this does not hold back"
                )
            } else {
                format!(
                    "{place} is over its cores: {load}; builds cargo sends its work to {}, which has idle cores, so this holds nothing back",
                    elsewhere.join(" and ")
                )
            }));
        } else if ready > 0 && five < IDLE_SHARE * cores {
            let items = crate::text::counted(ready, "item is ready", "items are ready");
            let place = crate::host::place(&host.name);
            match crate::host::takes_cargo(host, probe, floor_kib) {
                Ok(_) => found.push(Advice::said(format!(
                    "cores idle on {place} while {items}: {load}; start agents"
                ))),
                Err(why) => found.push(passed_over(&place, &items, &why)),
            }
        }
    }
    found
}

/// The line for an idle host that `builds cargo` passes over for <why>
/// (`host::takes_cargo`), with the fix for that reason: agents started for its
/// cores would build elsewhere, so health says what keeps work off it.
fn passed_over(place: &str, items: &str, why: &PassedOver) -> Advice {
    let said =
        format!("cores idle on {place} while {items}, but builds cargo passes it over: {why}");
    match why {
        PassedOver::UnderFloor { .. } => Advice::run(
            format!("{said}. Free disk there (builds help, DISK), starting with"),
            "builds prune".to_string(),
        ),
        PassedOver::GitFails { host, .. } => Advice::said(format!("{said}. Fix git on {host}")),
        PassedOver::NoSpeed { .. } | PassedOver::Unreachable { .. } => Advice::said(said),
    }
}

/// The project whose items go first: work on the builds tool slows every agent
/// until it lands, so load never holds its items back.
const BUILDS_PROJECT: &str = "builds";

/// The line for the ready items of project builds that no agent runs, whatever
/// the hosts' load; none when there are none.
pub fn builds_items_ready(loaded: &std::result::Result<Vec<Loaded>, String>) -> Option<Advice> {
    let titles: Vec<String> = loaded
        .iter()
        .flatten()
        .filter(|store| store.project == BUILDS_PROJECT)
        .filter_map(|store| store.lists.as_ref().ok())
        .flat_map(|lists| {
            crate::lists::derive::dispatch_order(lists, crate::lists::Audience::Coordinator)
        })
        .map(|item| format!("{} {}", item.id, item.title))
        .collect();
    (!titles.is_empty()).then(|| {
        Advice::said(format!(
            "builds items ready and not running: {}; start them (builds work goes first)",
            titles.join(", ")
        ))
    })
}

/// The items ready to dispatch (`builds next`) in every project whose store
/// loads. `lists::running::findings` names each store that fails to load.
pub fn ready_items(loaded: &std::result::Result<Vec<Loaded>, String>) -> usize {
    loaded
        .iter()
        .flatten()
        .filter_map(|store| store.lists.as_ref().ok())
        .map(|lists| {
            crate::lists::derive::dispatch_order(lists, crate::lists::Audience::Coordinator).len()
        })
        .sum()
}

#[cfg(test)]
#[path = "host_load_tests.rs"]
mod tests;
