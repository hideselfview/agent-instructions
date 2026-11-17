//! Sizing a lane's app checkouts by its queue. A person records a lane's
//! first checkouts with `builds app-checkout`; builds adds more on a host when
//! builds wait for the lane, and removes the ones it added once no build has
//! used them for a while, so their disk comes back.
//!
//! A build that joins a lane's queue and has to wait, or a build that may run
//! only on a host (an `opens-windows` test line) in a lane with no checkout on
//! a host, adds a checkout (`grow`): on the host with the most free cores, of
//! those that have a core free, disk free above the floor by one checkout of
//! the lane, and fewer than `HOST_CAP` app checkouts, while the lane has fewer
//! than `LANE_CAP`. builds names it `builds-<lane>-<n>` and records it in the
//! lane's set as its own, with when and why (lane.rs `AppCheckout`). Its first
//! build makes it on the host, as for a checkout `builds app-checkout --host`
//! records. builds adds checkouts only on hosts: this Mac is the person's, and
//! its disk holds every side worktree's builds.
//!
//! A build that joins a lane's queue, and `builds prune`, remove each
//! checkout builds added that no build has used for `IDLE_MINUTES`
//! (`shrink`): its worktree and build folders on the host first, holding its
//! lock so no build takes it meanwhile, then its line in the set, with a line
//! in the lane's removal record that says why. A checkout a person added
//! stays until a person removes it.
//!
//! Every read and change of the set that decides which checkouts there are
//! happens under the lane's queue guard (queue.rs `Queue::guard`), as does
//! every build's take of a lock (lock.rs), so two builds never add the same
//! checkout and no build takes one that is being removed. An addition on a
//! host also holds the host's guard (`Places::app_host_guard_file`) while it
//! counts the host's checkouts across repos and lanes and writes its own, so
//! builds of two lanes never pass `HOST_CAP` together. A build takes the
//! lane's guard first and the host's second, and nothing takes them the other
//! way round. The hosts are asked for their load and disk before either guard
//! is taken, since asking takes a second or more, and the set is read again
//! under the lane's guard.

use crate::app_remote::{RemoteApp, entry_of, split};
use crate::fail;
use crate::fail::{Context, Result};
use crate::holder_run::HolderRun;
use crate::host::{self, Host, Probe, Ssh};
use crate::lane::{AddedBy, AppCheckout, Lane, Removed};
use crate::places::Places;
use crate::prune::Run;
use crate::queue::{self, Queue};
use crate::text::{human, minutes_since, note};

/// The most app checkouts a lane has. On 2026-11-16, the busiest day
/// recorded (85 android and 101 mac runs), 4 builds at once, waiting or
/// building, covered 97% of the minutes the mac lane had any and 90% of the
/// android lane's.
pub const LANE_CAP: usize = 4;

/// The most app checkouts one host holds, across repos and lanes. On the same
/// day, with each lane held to `LANE_CAP`, 6 builds at once covered 89% of
/// the minutes either lane had any.
pub const HOST_CAP: usize = 6;

/// How long a checkout builds added may go without a build before builds
/// removes it. On 2026-11-16, 96% of the gaps between two builds in one
/// checkout were under 30 minutes and 98% under 60; the longer ones were the
/// ends of a day's work.
pub const IDLE_MINUTES: i64 = 60;

/// The cores a host must have free (its cores less its one-minute load
/// average) to take a new checkout: a host with no core free would only slow
/// the builds already there.
const FREE_CORES: f64 = 1.0;

/// How long the removal record keeps a removal.
const REMOVED_KEPT_SECONDS: i64 = 12 * 3600;

/// About how much disk one built checkout of <lane> takes. On the M1 on
/// 2026-11-16, a mac lane checkout held 22 GiB (target-macos 9.0, bae-macos
/// 10.1, BaeKit 1.7, target 0.7) and an android lane checkout 10 GiB
/// (target-android 8.0, bae-android 0.9, target 0.7).
pub fn checkout_kib(lane: Lane) -> u64 {
    const GIB: u64 = 1024 * 1024;
    match lane {
        Lane::Mac => 22 * GIB,
        Lane::Android => 10 * GIB,
    }
}

/// One repo's app lane, and the program that reaches its hosts: what sizing
/// reads and changes.
#[derive(Clone, Copy)]
pub struct RepoLane<'a> {
    pub places: &'a Places,
    pub ssh: &'a Ssh,
    pub repo: &'a str,
    pub lane: Lane,
}

/// Why a build asks for a checkout.
#[derive(Clone, Copy, Debug)]
pub enum Need {
    /// It joined the queue and waits behind the builds that hold the lane's
    /// locks. builds adds a checkout while any build waits.
    Waits,
    /// It may run only on a host, and the lane has no checkout on one.
    OnAHost,
}

/// Adds a checkout of <at>'s lane on a host for a build that <need>s one,
/// when a host and the caps allow it, and returns its entry. A host whose
/// free disk would go under <floor_kib> takes none. When builds adds none, it
/// says why.
pub fn grow(at: RepoLane, floor_kib: u64, need: Need) -> Result<Option<String>> {
    let RepoLane {
        places,
        ssh,
        repo,
        lane,
    } = at;
    let hosts = host::read_remote(places)?;
    if hosts.is_empty() {
        return Ok(None);
    }
    let probes = host::probe_all(places, ssh, hosts);
    let refusing: Vec<String> = if lane.uses_xcode() {
        host::refusing_mac_builds(places)?
            .into_iter()
            .map(|refusal| refusal.host)
            .collect()
    } else {
        Vec::new()
    };
    let queue = Queue::of(places, repo, lane);
    let lane_guard = queue.guard()?;
    let mut set = places.app_checkouts_recorded(repo, lane)?;
    let order = queue.order();
    let reason = match need {
        Need::Waits => {
            // Not only this build: of two that wait, the one that adds first
            // may add the checkout the other is let in for.
            let slots = set.len();
            if order.len() <= slots {
                return Ok(None);
            }
            format!(
                "{} for the {} lane's {}",
                crate::text::counted(order.len() - slots, "build waited", "builds waited"),
                lane.word(),
                crate::text::counted(slots, "checkout", "checkouts")
            )
        }
        Need::OnAHost => {
            if set.iter().any(|checkout| split(&checkout.entry).is_some()) {
                return Ok(None);
            }
            format!(
                "a build that opens windows needed a {} lane checkout on a host",
                lane.word()
            )
        }
    };
    if set.len() >= LANE_CAP {
        note(format!(
            "{repo}'s {} lane has {LANE_CAP} app checkouts, the most it has, so builds adds none",
            lane.word()
        ));
        return Ok(None);
    }
    let (fits, mut lacks) = fitting_hosts(lane, floor_kib, &probes, &refusing);
    for (host, probe) in fits {
        let host_guard = queue::guard_at(&places.app_host_guard_file(&host.name))?;
        let held = checkouts_on(places, &host.name)?;
        if held >= HOST_CAP {
            lacks.push(format!(
                "{} holds {held} app checkouts, the most a host holds",
                host.name
            ));
            continue;
        }
        let taken: Vec<String> = Lane::ALL
            .into_iter()
            .map(|lane| places.app_checkouts_of(repo, lane))
            .collect::<Result<Vec<_>>>()?
            .concat();
        let entry = (1..)
            .map(|n| entry_of(&host.name, &format!("builds-{}-{n}", lane.word())))
            .find(|entry| !taken.contains(entry))
            .expect("some number names no checkout");
        let why = format!(
            "{reason}, and host {} had {:.1} cores and {} of disk free",
            host.name,
            probe.room(),
            human(probe.free_kib)
        );
        set.push(AppCheckout {
            entry: entry.clone(),
            added_by: AddedBy::Builds {
                at: crate::app_timing::now(),
                why: why.clone(),
            },
        });
        places.write_app_checkouts(repo, lane, &set)?;
        drop(host_guard);
        drop(lane_guard);
        queue::repoint(places, repo, lane);
        note(format!(
            "builds added {entry} to {repo}'s {} app checkouts: {why}; its first build makes it on host {}",
            lane.word(),
            host.name
        ));
        return Ok(Some(entry));
    }
    note(format!(
        "no host can take another {} lane checkout of {repo}: {}",
        lane.word(),
        lacks.join("; ")
    ));
    Ok(None)
}

/// The hosts that may take a checkout of <lane>, the one with the most cores
/// free first: of the hosts in <probes>, those that answer, take jobs, are
/// not in <refusing>, have `FREE_CORES` free and would keep their free disk
/// above <floor_kib> with one more checkout of the lane. With them, what each
/// other host lacks. The caller checks `HOST_CAP` under the host's guard.
fn fitting_hosts(
    lane: Lane,
    floor_kib: u64,
    probes: &[(Host, std::result::Result<Probe, String>)],
    refusing: &[String],
) -> (Vec<(Host, Probe)>, Vec<String>) {
    let mut fits: Vec<(Host, Probe)> = Vec::new();
    let mut lacks = Vec::new();
    for (host, probe) in probes {
        if let Some(why) = host::passed_over(host, probe, floor_kib) {
            lacks.push(why);
            continue;
        }
        let Ok(probe) = probe else {
            unreachable!("passed_over passes over a host that does not answer");
        };
        if refusing.contains(&host.name) {
            lacks.push(format!(
                "{} cannot build the mac lane (`builds health` says why)",
                host.name
            ));
        } else if probe.room() < FREE_CORES {
            lacks.push(format!(
                "{} has {:.1} cores free, under {FREE_CORES:.0}",
                host.name,
                probe.room()
            ));
        } else if probe.free_kib < floor_kib + checkout_kib(lane) {
            lacks.push(format!(
                "{} has {} free, and a {} lane checkout of about {} would take it under the floor of {}",
                host.name,
                human(probe.free_kib),
                lane.word(),
                human(checkout_kib(lane)),
                human(floor_kib)
            ));
        } else {
            fits.push((host.clone(), probe.clone()));
        }
    }
    fits.sort_by(|a, b| b.1.room().total_cmp(&a.1.room()));
    (fits, lacks)
}

/// How many app checkouts, of every repo's lanes on the volume, are on
/// <host>.
fn checkouts_on(places: &Places, host_name: &str) -> Result<usize> {
    let mut count = 0;
    for repo in places.repo_folder_names()? {
        for lane in Lane::ALL {
            count += places
                .app_checkouts_of(&repo, lane)?
                .iter()
                .filter(|entry| split(entry).is_some_and(|(host, _)| host == host_name))
                .count();
        }
    }
    Ok(count)
}

/// When a build last used <entry>, a checkout of <at>'s lane that builds
/// added at <added_at>: the latest of that time, when the last build let its
/// lock go, and the times in its last holder's record of its run (for a build
/// that was killed before it could write when it let go). Seconds since 1970.
fn last_use(at: RepoLane, entry: &str, added_at: i64) -> Result<i64> {
    let RepoLane {
        places, repo, lane, ..
    } = at;
    let mut last = added_at;
    let ended = places.app_lock_ended_file(repo, lane, entry);
    match std::fs::read_to_string(&ended) {
        Ok(text) => match text.trim().parse::<i64>() {
            Ok(at) => last = last.max(at),
            Err(_) => fail!("{} holds no time: {text:?}", ended.display()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => fail!("reading {}: {error}", ended.display()),
    }
    if let Some(run) = HolderRun::read(&places.app_lock_run_file(repo, lane, entry))? {
        last = last.max(run.joined_at + (run.waited_ms / 1000) as i64);
        if let Some(step) = run.step {
            last = last.max(step.started_at);
        }
    }
    Ok(last)
}

/// Removes each checkout of <at>'s lane that builds added and no build has
/// used for `IDLE_MINUTES`, or with <run> dry, says which it would remove. A
/// checkout whose host does not answer, or whose removal there fails, stays
/// in the set, said, and goes the next time.
pub fn shrink(at: RepoLane, run: Run) -> Result<()> {
    let RepoLane {
        places, repo, lane, ..
    } = at;
    let now = crate::app_timing::now();
    for checkout in places.app_checkouts_recorded(repo, lane)? {
        let AddedBy::Builds { at: added_at, .. } = checkout.added_by else {
            continue;
        };
        let dir = places.app_locks_dir(repo, lane);
        std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
        let entry = checkout.entry;
        let lock_file = places.app_lock_file(repo, lane, &entry);
        let Some(lock) = queue::try_lock(&lock_file)? else {
            // A build holds it.
            continue;
        };
        let removed = remove_if_idle(at, &entry, Idle { added_at, now }, run);
        unsafe { libc::close(lock) };
        removed?;
    }
    Ok(())
}

/// The times `shrink` judges a checkout builds added by, in seconds since
/// 1970: when builds added it, and now.
#[derive(Clone, Copy)]
struct Idle {
    added_at: i64,
    now: i64,
}

/// `shrink` for one checkout of <at>'s lane, <entry>, whose lock the caller
/// holds.
fn remove_if_idle(at: RepoLane, entry: &str, times: Idle, run: Run) -> Result<()> {
    let RepoLane {
        places,
        ssh,
        repo,
        lane,
    } = at;
    let Idle { added_at, now } = times;
    let idle = minutes_since(now, last_use(at, entry, added_at)?);
    if idle < IDLE_MINUTES {
        return Ok(());
    }
    let why = format!("no build used it for {idle} minutes");
    let shown = format!("{entry} from {repo}'s {} app checkouts", lane.word());
    if run.dry() {
        note(format!("would remove {shown} ({why})"));
        return Ok(());
    }
    let removed_there = match RemoteApp::find(places, ssh, repo, lane, entry) {
        Ok(Some(remote)) => remote.remove(),
        Ok(None) => Err(format!("{entry} is not on a host")),
        Err(error) => Err(error.text()),
    };
    if let Err(said) = removed_there {
        note(format!("kept {shown}, which {why}: {said}"));
        return Ok(());
    }
    let queue = Queue::of(places, repo, lane);
    {
        let _guard = queue.guard()?;
        let mut set = places.app_checkouts_recorded(repo, lane)?;
        set.retain(|checkout| checkout.entry != entry);
        places.write_app_checkouts(repo, lane, &set)?;
        let mut removals: Vec<Removed> = places
            .app_checkouts_removed(repo, lane)?
            .into_iter()
            .filter(|removal| removal.at > now - REMOVED_KEPT_SECONDS)
            .collect();
        removals.push(Removed {
            at: now,
            entry: entry.to_string(),
            why: why.clone(),
        });
        let text: String = removals
            .iter()
            .map(|removal| format!("{}\n", removal.line()))
            .collect();
        crate::whole_file::write(&places.app_checkouts_removed_file(repo, lane), text)?;
        for file in [
            places.app_lock_file(repo, lane, entry),
            places.app_lock_run_file(repo, lane, entry),
            places.app_lock_ended_file(repo, lane, entry),
        ] {
            remove_file(&file)?;
        }
        let folder = places.root.join(repo).join(entry);
        match std::fs::remove_dir_all(&folder) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("removing {}: {error}", folder.display())
            }
            _ => {}
        }
    }
    queue::repoint(places, repo, lane);
    note(format!(
        "removed {shown} and its worktree and build folders on the host: {why}"
    ));
    Ok(())
}

fn remove_file(file: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(file) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            fail!("removing {}: {error}", file.display())
        }
        _ => Ok(()),
    }
}

/// `shrink` for every lane of every repo on the volume, for `builds prune`.
pub fn shrink_all(places: &Places, ssh: &Ssh, run: Run) -> Result<()> {
    for repo in places.repo_folder_names()? {
        for lane in Lane::ALL {
            let at = RepoLane {
                places,
                ssh,
                repo: &repo,
                lane,
            };
            shrink(at, run)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "lane_size_tests.rs"]
mod tests;
