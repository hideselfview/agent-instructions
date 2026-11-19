//! What `builds health` fixes before it lists what needs acting on: each
//! dispatch blocker that a command fixes with no judgment call. A build host
//! under the free-disk floor gets the prune `builds prune` runs, a host with
//! no measured core speed gets its speed measured (`builds host setup`, without
//! its sudo steps), and a claim on a worktree that nothing holds is released.
//! Each fix says in one line what it did. Health never runs sudo and stops a
//! process only with `--stop-junk`. A host that does not answer, a host whose
//! git fails and a worktree with uncommitted work stay findings, with the
//! reason, because they need a person's judgment.

use crate::claim::{Claim, STARTING};
use crate::fail::Result;
use crate::host::{Host, PassedOver, Probe};
use crate::text::human;
use crate::tool::Tool;

/// What probing a host said.
pub type Probed = std::result::Result<Probe, String>;

/// How health fixes a host: `cmd_health` gives the real commands, and a test
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
        if let Some(PassedOver::UnderFloor { free_kib, .. }) =
            crate::host::takes_cargo(host, probe, floor_kib).err()
        {
            let had = human(free_kib);
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
            && let Some(PassedOver::NoSpeed { .. }) =
                crate::host::takes_cargo(host, probe, floor_kib).err()
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

/// Releases each claim on a side worktree that nothing holds: not an app
/// checkout's, not held (`builds note --state held`), not one made for an item
/// that `builds item take` has not yet started there (`claim::STARTING`), and
/// with nothing that keeps the worktree (`kept::Kept::is_empty`: no item runs
/// or is kept there, no commit that origin/main lacks, no file not committed,
/// no `git commit` running). Returns one line for each worktree it released.
pub fn release_idle_claims(tool: &Tool) -> Result<Vec<String>> {
    let every = crate::lists::store::every_lists(&tool.places)?;
    let commits = crate::procs::git_commits(&crate::procs::Processes::read()?);
    let mut lines = Vec::new();
    for root in tool.roots()? {
        let Some(checkout) = &root.live else {
            continue;
        };
        if !checkout.is_side() || root.is_app_checkout(&tool.places)? {
            continue;
        }
        let Some(claim) = Claim::read(&checkout.claim_file())? else {
            continue;
        };
        if matches!(claim.note_state.as_deref(), Some("held" | STARTING)) {
            continue;
        }
        if crate::kept::read(&every, checkout, &commits)?.is_empty()
            && let Some(claim) = crate::claim::forget(checkout)?
        {
            lines.push(format!(
                "released {}, which nothing held (was {})",
                checkout.top.display(),
                claim.text()
            ));
        }
    }
    Ok(lines)
}

#[cfg(test)]
#[path = "health_fix_tests.rs"]
mod tests;
