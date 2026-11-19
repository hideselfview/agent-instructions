//! Whether the build hosts' cores do the work that waits. While items are
//! ready to dispatch, the coordinator keeps every host's cores busy, and while
//! a host's load is well over its cores, it starts nothing new, except items of project builds. `builds health`
//! lists a host whose cores sit idle while items are ready, and a host over its
//! cores, under "needs action", and every ready item of project builds that
//! no agent runs.
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
                "{} is over its cores: {load}; start nothing new, except builds items, which this does not hold back",
                crate::host::place(&host.name)
            )));
        } else if ready > 0 && five < IDLE_SHARE * cores {
            let items = crate::text::counted(ready, "item is ready", "items are ready");
            let place = crate::host::place(&host.name);
            match crate::host::takes_cargo(host, probe, floor_kib) {
                Ok(_) => found.push(Advice::said(format!(
                    "cores idle on {place} while {items}: {load}; start agents"
                ))),
                Err(why) => found.push(passed_over(host, answer, floor_kib, &place, &items, &why)),
            }
        }
    }
    found
}

/// The line for an idle host that `builds cargo` passes over for <why> (what
/// `host::takes_cargo` said), with the fix for that reason: agents started for
/// its cores would build elsewhere, so health says what keeps work off it.
fn passed_over(
    host: &Host,
    answer: &Probe,
    floor_kib: u64,
    place: &str,
    items: &str,
    why: &str,
) -> Advice {
    let name = &host.name;
    let said =
        format!("cores idle on {place} while {items}, but builds cargo passes it over: {why}");
    if answer.free_kib < floor_kib {
        Advice::run(
            format!("{said}. Free disk there (builds help, DISK), starting with"),
            "builds prune".to_string(),
        )
    } else if answer.git_fails.is_some() {
        Advice::said(format!("{said}. Fix git on {name}"))
    } else {
        Advice::said(said)
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
