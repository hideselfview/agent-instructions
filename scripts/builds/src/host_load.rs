//! Whether the build hosts' cores do the work that waits. While items are
//! ready to dispatch, the coordinator keeps every host's load in its band, and
//! while every host that takes cargo work is over its band, it starts nothing
//! new, except items of project builds. `builds health` lists a host whose
//! load is under its band while items are ready, and a host over its band,
//! under "needs action", and every ready item of project builds that no agent
//! runs. A host over its band holds nothing back while another host that takes
//! cargo work is under its band, because `builds cargo` sends the work there.
//!
//! A host's band is the one `builds host band` set (host_band.rs), or the
//! shares of its cores when it has none. A host whose kernel reports memory
//! pressure at warning or critical is over its band whatever its load
//! (host_memory.rs), and its line says its memory next to its load.
//!
//! A host whose CPU sits under SATURATED_IDLE_PERCENT idle (host_scan.rs
//! reads it over one second) has no room, whatever its load: processes that
//! wait on the disk or on a lock add to the load average without using a
//! core, so a load under the band does not mean that cores sit idle. Such a
//! host is never under its band, and while items are ready its line says it
//! has no room in place of "start agents". Each line says the host's idle
//! share next to its load.
//!
//! Both judge the 5-minute load average: health runs every 5 minutes, and the
//! 1-minute average rises and falls with each build's steps.

use crate::health::Advice;
use crate::host::{Host, PassedOver, Probe};
use crate::lists::store::Loaded;

/// A host whose CPU sits idle less than this share, as a percentage, has no
/// room for more work.
pub const SATURATED_IDLE_PERCENT: f64 = 5.0;

/// A host as health measures it: what its probe said, and the share of its
/// CPU that sat idle, as a percentage (host_scan.rs), or why health could not
/// read either.
pub struct Measured {
    pub host: Host,
    pub probe: std::result::Result<Probe, String>,
    pub idle: std::result::Result<f64, String>,
}

impl Measured {
    /// Whether the host's CPU sits under SATURATED_IDLE_PERCENT idle.
    fn saturated(&self) -> bool {
        self.idle
            .as_ref()
            .is_ok_and(|idle| *idle < SATURATED_IDLE_PERCENT)
    }

    /// `CPU 0% idle`, or why health could not read it.
    fn idle_text(&self) -> String {
        match &self.idle {
            Ok(idle) => format!("CPU {idle:.0}% idle"),
            Err(error) => format!("CPU idle share unknown ({error})"),
        }
    }
}

/// The lines for <hosts> while <ready> items are ready to dispatch: each host
/// that did not answer, each host over its band, and each host under its
/// band while items are ready. A host that `builds cargo` can give work to
/// (`host::takes_cargo`, with <floor_kib> of free disk) gets "start agents"
/// when its CPU has room, and the line that says it has none when its CPU is
/// saturated; any other gets the reason `builds cargo` passes it over and the
/// fix, because agents started for its cores would build elsewhere.
pub fn findings(hosts: &[Measured], ready: usize, floor_kib: u64) -> Vec<Advice> {
    let mut found = Vec::new();
    // The hosts `builds cargo` can give work to whose 5-minute load is under
    // their band and whose CPU has room, each with its place: while one
    // exists, the others' load holds back nothing, since `builds cargo` sends
    // the work there.
    let roomy: Vec<(&str, String)> = hosts
        .iter()
        .filter_map(|measured| {
            let Measured { host, probe, .. } = measured;
            let answer = probe.as_ref().ok()?;
            let idle = !over(host, answer)
                && !measured.saturated()
                && answer.load[1] < host.band_for(answer.cores).low;
            (idle && crate::host::takes_cargo(host, probe, floor_kib).is_ok())
                .then(|| (host.name.as_str(), crate::host::place(&host.name)))
        })
        .collect();
    for measured in hosts {
        let Measured { host, probe, .. } = measured;
        let answer = match probe {
            Ok(answer) => answer,
            Err(error) => {
                found.push(Advice::said(format!(
                    "{} did not say its load, so health cannot tell whether it is under or over its band: {error}",
                    crate::host::place(&host.name)
                )));
                continue;
            }
        };
        let five = answer.load[1];
        let band = host.band_for(answer.cores);
        let load = format!(
            "its 5-minute load is {five:.1} on {} cores, against a band of {}; {}; {}",
            answer.cores,
            band.text(),
            measured.idle_text(),
            answer.memory.text()
        );
        let place = crate::host::place(&host.name);
        // What holds the host back, said after its place, when it may not
        // take more work: over its band, or with no room on its CPU.
        let held = if over(host, answer) {
            Some(if answer.memory.under_pressure() && five <= band.high {
                "is under memory pressure, which puts it over its band whatever its load"
                    .to_string()
            } else {
                "is over its band".to_string()
            })
        } else if ready > 0 && five < band.low && measured.saturated() {
            Some(format!(
                "is under its band by load, but its CPU sits under {SATURATED_IDLE_PERCENT:.0}% idle, so it has no room while {}",
                crate::text::counted(ready, "item is ready", "items are ready")
            ))
        } else {
            None
        };
        if let Some(why) = held {
            let elsewhere: Vec<&str> = roomy
                .iter()
                .filter(|(name, _)| *name != host.name)
                .map(|(_, place)| place.as_str())
                .collect();
            found.push(Advice::said(if elsewhere.is_empty() {
                format!(
                    "{place} {why}: {load}; start nothing new, except builds items, which this does not hold back"
                )
            } else {
                format!(
                    "{place} {why}: {load}; builds cargo sends its work to {}, which is under its band, so this holds nothing back",
                    elsewhere.join(" and ")
                )
            }));
        } else if ready > 0 && five < band.low {
            let items = crate::text::counted(ready, "item is ready", "items are ready");
            match crate::host::takes_cargo(host, probe, floor_kib) {
                Ok(_) => found.push(Advice::said(format!(
                    "{place} is under its band while {items}: {load}; start agents"
                ))),
                Err(why) => found.push(passed_over(&place, &items, &why)),
            }
        }
    }
    found
}

/// Whether <host> is over its band: its 5-minute load is above the band's
/// high end, or the kernel reports memory pressure at warning or critical.
fn over(host: &Host, answer: &Probe) -> bool {
    answer.load[1] > host.band_for(answer.cores).high || answer.memory.under_pressure()
}

/// The line for an idle host that `builds cargo` passes over for <why>
/// (`host::takes_cargo`), with the fix for that reason: agents started for its
/// cores would build elsewhere, so health says what keeps work off it.
fn passed_over(place: &str, items: &str, why: &PassedOver) -> Advice {
    let said =
        format!("{place} is under its band while {items}, but builds cargo passes it over: {why}");
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
