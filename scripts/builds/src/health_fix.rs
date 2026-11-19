//! What `builds health` fixes before it lists what needs acting on: each
//! dispatch blocker that a command fixes with no judgment call. A build host
//! under the free-disk floor gets the prune `builds prune` runs, a host with
//! no measured core speed gets its speed measured (`builds host setup`, without
//! its sudo steps), and a claim on a worktree that nothing holds is released.
//! Each fix says in one line what it did. Health never runs sudo and stops a
//! process only with `--stop-junk`. A host that does not answer, a host whose
//! git fails and a worktree with uncommitted work stay findings, with the
//! reason, because they need a person's judgment.

use crate::claim::Claim;
use crate::fail::Result;
use crate::host::{Host, Probe};
use crate::text::human;
use crate::tool::Tool;

/// A claim younger than this many minutes is left alone: `builds claim` comes
/// before the `builds item take` that gives the claim its item, and health
/// must not release a claim in between.
const NEW_CLAIM_MINUTES: i64 = 10;

/// What probing a host said.
pub type Probed = std::result::Result<Probe, String>;

/// How health fixes a host. `Fixes::of` runs the real commands, and a test
/// gives its own.
pub struct Fixes<'a> {
    /// Runs the prune on a host, or says why it failed.
    pub prune: &'a dyn Fn(&Host) -> std::result::Result<(), String>,
    /// Probes a host again.
    pub probe: &'a dyn Fn(&Host) -> Probed,
    /// Measures a host's core speed against this Mac's and records it.
    pub measure: &'a dyn Fn(&Host) -> std::result::Result<f64, String>,
}

/// Fixes each host of <hosts> that `host::takes_cargo` passes over for a
/// reason builds can fix, with <fixes>. <probes> holds each host's probe, in
/// the order of <hosts>. A host it prunes gets its probe read again, and a
/// host it measures gets its speed. Returns one line for each fix and each
/// fix that failed.
pub fn fix_hosts(
    hosts: &mut [Host],
    probes: &mut [Probed],
    floor_kib: u64,
    fixes: &Fixes,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (host, probe) in hosts.iter_mut().zip(probes.iter_mut()) {
        let name = host.name.clone();
        let place = crate::host::place(&name);
        let under = probe
            .as_ref()
            .ok()
            .map(|probe| probe.free_kib)
            .filter(|free| *free < floor_kib);
        if let Some(free) = under {
            let had = human(free);
            match (fixes.prune)(host) {
                Ok(()) => {
                    *probe = (fixes.probe)(host);
                    lines.push(match probe {
                        Ok(after) => format!(
                            "pruned {place}, which had {had} free, under the floor of {}: it has {} free now",
                            human(floor_kib),
                            human(after.free_kib)
                        ),
                        Err(error) => format!(
                            "pruned {place}, which had {had} free, under the floor of {}, but it did not answer the next probe: {error}",
                            human(floor_kib)
                        ),
                    });
                }
                Err(said) if said.contains(crate::prune_schedule::BUSY) => lines.push(format!(
                    "{place} has {had} free, under the floor of {}: already pruning",
                    human(floor_kib)
                )),
                Err(said) => lines.push(format!("pruning {place} failed: {said}")),
            }
        }
        if host.target.is_some()
            && host.speed.is_none()
            && crate::host::passed_over(host, probe, floor_kib).is_none()
        {
            match (fixes.measure)(host) {
                Ok(speed) => {
                    host.speed = Some(speed);
                    lines.push(format!(
                        "measured the core speed of {place}: {speed:.2} of this Mac's"
                    ));
                }
                Err(said) => lines.push(format!(
                    "measuring the core speed of {place} failed: {said}"
                )),
            }
        }
    }
    lines
}

/// Releases each claim on a side worktree that health would tell the
/// coordinator to release (`Worktree::claim_is_idle`), once `builds release`
/// would accept it too. Returns one line for each worktree it released.
pub fn release_idle_claims(tool: &Tool) -> Result<Vec<String>> {
    let every = crate::lists::store::every_lists(&tool.places)?;
    let commits = crate::procs::git_commits(&crate::procs::Processes::read()?);
    let mut lines = Vec::new();
    for root in tool.roots()? {
        let Some(checkout) = &root.live else {
            continue;
        };
        if !checkout.is_side() {
            continue;
        }
        let Some(claim) = Claim::read(&checkout.claim_file())? else {
            continue;
        };
        if crate::text::minutes_since(tool.now, claim.since) < NEW_CLAIM_MINUTES {
            continue;
        }
        let top = checkout.top.display().to_string();
        let git = crate::side_git::read(&checkout.top);
        let worktree = crate::lists::held::Worktree::read(
            &tool.places,
            top.clone(),
            checkout,
            &claim,
            &git,
            &commits,
        )?;
        let nothing_runs = crate::lists::running::running_in_any(&every, &top).is_empty();
        if worktree.claim_is_idle(nothing_runs)
            && crate::kept::read(&every, checkout, &commits)?.is_leavable()
            && let Some(claim) = crate::claim::forget(checkout)?
        {
            lines.push(format!(
                "released {top}, which nothing held (was {})",
                claim.text()
            ));
        }
    }
    Ok(lines)
}

#[cfg(test)]
#[path = "health_fix_tests.rs"]
mod tests;
