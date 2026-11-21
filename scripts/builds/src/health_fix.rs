//! What `builds health` fixes before it lists what needs acting on: each
//! dispatch blocker that a command fixes with no judgment call. A build host
//! under the free-disk floor gets the prune `builds prune` runs, a host with
//! no measured core speed gets its speed measured (`builds host setup`, without
//! its sudo steps), a host under memory pressure gets the build daemons that
//! no build uses stopped, and a claim on a worktree that nothing holds is
//! released, and a process whose caller is gone and whose only work is what
//! it prints is stopped (orphans.rs).
//! Each fix says in one line what it did. Health never runs sudo, and stops
//! no other process without `--stop-junk`. A host that does not answer, a
//! host whose git fails and a worktree with uncommitted work stay findings,
//! with the reason, because they need a person's judgment.

use crate::claim::Claim;
use crate::fail::Result;
use crate::host::{Host, PassedOver, Probe};
use crate::text::human;
use crate::tool::Tool;

/// What probing a host said.
pub type Probed = std::result::Result<Probe, String>;

/// The lines for the daemons a stop tried, or why it could not try.
pub type Stopped = std::result::Result<Vec<String>, String>;

/// How health fixes a host: `cmd_health` gives the real commands, and a test
/// gives its own.
pub struct Fixes<'a> {
    /// Runs the prune on a host, or says why it failed.
    pub prune: &'a dyn Fn(&Host) -> std::result::Result<(), String>,
    /// Probes a host again.
    pub probe: &'a dyn Fn(&Host) -> Probed,
    /// Measures a host's core speed against this Mac's and records it.
    pub measure: &'a dyn Fn(&Host) -> std::result::Result<f64, String>,
    /// Stops the build daemons on a host that no build uses (idle_daemons.rs),
    /// given the host's memory as `Memory::text` says it, and returns one line
    /// for each daemon it tried to stop.
    pub stop_idle: &'a dyn Fn(&Host, &str) -> Stopped,
}

/// Fixes each host of <hosts> that `host::takes_cargo` passes over for a
/// reason builds can fix, with <fixes>. <probes> holds each host's probe, in
/// the order of <hosts>. A host it prunes gets its probe read again, and a
/// host it measures gets its speed. A host under memory pressure gets its
/// idle build daemons stopped, and its probe read again when it stopped any,
/// before anything judges whether the host takes work. Returns one line for each fix and each
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
        let pressure = probe
            .as_ref()
            .ok()
            .filter(|probe| probe.memory.under_pressure())
            .map(|probe| probe.memory.text());
        if let Some(pressure) = pressure {
            match (fixes.stop_idle)(host, &pressure) {
                Ok(stopped) if stopped.is_empty() => {}
                Ok(stopped) => {
                    lines.extend(stopped);
                    *probe = (fixes.probe)(host);
                }
                Err(said) => lines.push(format!(
                    "stopping the idle build daemons of {place} failed: {said}"
                )),
            }
        }
    }
    lines
}

/// Stops each of <orphans> that health stops, with <stop>, which runs an
/// orphan's `stop_script` on its host and returns what it printed or why it
/// could not run it. An orphan is stopped when the script names no target
/// that refused its signal. Returns one line for each orphan it stopped and
/// each stop that failed, and the orphans left to list: those health leaves
/// to run and those whose stop failed.
pub fn stop_orphans(
    orphans: Vec<crate::orphans::Orphan>,
    stop: &dyn Fn(&Host, &str) -> std::result::Result<String, String>,
) -> (Vec<String>, Vec<crate::orphans::Orphan>) {
    let mut lines = Vec::new();
    let mut left = Vec::new();
    for orphan in orphans {
        let Some(script) = orphan.stop_script() else {
            left.push(orphan);
            continue;
        };
        let refused = stop(&orphan.host, &script).map(|printed| {
            printed
                .lines()
                .filter_map(|line| line.strip_prefix("refused="))
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
        match refused {
            Ok(refused) if refused.is_empty() => {
                lines.push(format!("stopped {}: its caller was gone", orphan.what()))
            }
            Ok(refused) => {
                lines.push(format!(
                    "stopping {} failed: {} still {} and refused the signal",
                    orphan.what(),
                    refused.join(", "),
                    if refused.len() == 1 { "runs" } else { "run" }
                ));
                left.push(orphan);
            }
            Err(said) => {
                lines.push(format!("stopping {} failed: {said}", orphan.what()));
                left.push(orphan);
            }
        }
    }
    (lines, left)
}

/// Releases each claim that health would tell the coordinator to release
/// (`Worktree::claim_is_idle`, the test health's own finding uses): on a side
/// worktree that is not an app checkout's, not held and not waiting for its
/// item, where no item runs, no commit origin/main lacks waits, no file is
/// uncommitted and no `git commit` runs. Returns one line for each worktree it
/// released.
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
        let top = checkout.top.display().to_string();
        let worktree = crate::lists::held::Worktree::read(
            &tool.places,
            top.clone(),
            checkout,
            &claim,
            checkout.branch()?,
            &crate::side_git::read(&checkout.top),
            &commits,
        )?;
        let nothing_runs = crate::lists::running::running_in_any(&every, &top).is_empty();
        if worktree.claim_is_idle(nothing_runs)
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

/// Tells each project's judge again of each finding that waits and that no
/// running builds process is telling the judge's session of,
/// tells each judge that has not acknowledged the text to acknowledge it
/// (review/judge_ack.rs), and forgets each judge whose session ended, so that the coordinator
/// settles the findings that wait (review/judge_told.rs). Returns one line
/// for each message it sent, each judge it forgot and each store it could not
/// read or send for.
pub fn tell_judges(tool: &Tool) -> Result<Vec<String>> {
    let sessions = tool.places.claude_sessions();
    let stamp = crate::lists::Stamp {
        at: tool.now,
        by: "health".to_string(),
    };
    let mut lines = Vec::new();
    for store in crate::lists::store::stores(&tool.places)? {
        let project = store.project.clone();
        let done = crate::review::judge_told::forget_ended(&store, &stamp, &sessions)
            .and_then(|forgot| {
                lines.extend(forgot);
                crate::review::judge_told::tell_waiting(&store, &stamp, &sessions)
            })
            .and_then(|mut told| {
                told.extend(crate::review::judge_ack::tell_outdated(&store, &sessions)?);
                Ok(told)
            });
        match done {
            Ok(told) => lines.extend(told),
            Err(error) => lines.push(format!(
                "could not tell project {project}'s judge of the findings that wait: {}",
                error.text()
            )),
        }
    }
    Ok(lines)
}

#[cfg(test)]
#[path = "health_fix_tests.rs"]
mod tests;
