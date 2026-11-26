//! Sizing a lane's app checkouts by its queue. A person records a lane's
//! first checkouts with `builds app-checkout`; builds adds more on a host when
//! builds wait for the lane, and removes the ones it added once no build has
//! used them for a while, so their disk comes back.
//!
//! A build that joins a lane's queue and has to wait adds a checkout (`grow`): on the host with the most free cores, of
//! those that have a core free, disk free above the floor by one checkout of
//! the lane, and fewer than `HOST_CAP` app checkouts, while the lane has fewer
//! than `LANE_CAP`. builds names it `builds-<lane>-<n>` and records it in the
//! lane's set as its own, with when and why (lane.rs `AppCheckout`). Its first
//! build makes it on the host, as for a checkout `builds app-checkout --host`
//! records, and starts its cargo build folders from clones of the lane's
//! warmest checkout on that host (app_clone.rs), which is why a host with such
//! a checkout needs less free disk (`new_checkout_kib`). A waiting build adds checkouts only on hosts: this Mac is the
//! person's, and its disk holds every side worktree's builds. A build that
//! may run only on this Mac, in a lane with no checkout here, makes one local
//! checkout (worktree.rs `ensure_local_app_checkout`) that stays.
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
//! checkout and no build takes one that `shrink` is removing. An addition on a
//! host also holds the host's guard (`Places::app_host_guard_file`) while it
//! counts the host's checkouts across repos and lanes and writes its own, so
//! builds of two lanes never pass `HOST_CAP` together. A build takes the
//! lane's guard first and the host's second, and nothing takes them the other
//! way round. A build asks the hosts for their load and disk before it takes
//! either guard, since asking takes a second or more, and reads the set again
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

/// How much of `checkout_kib` a new checkout of <lane> shares with the warmest
/// checkout it is cloned from (warm.rs `clone_lane`): its cargo folders,
/// target-macos 9.0 GiB and the host target 0.7 for the mac lane,
/// target-android 8.0 and the host target 0.7 for the android lane, as
/// measured with `checkout_kib`. A clone shares every block with its source
/// until a build rewrites it, and a new checkout's first build rewrites only
/// the units of the crates that changed, so the clone adds none of these
/// blocks when the host is asked; `checkout_kib` less this is what the
/// checkout adds before its first build.
pub fn cloned_kib(lane: Lane) -> u64 {
    const GIB_TENTHS: u64 = 1024 * 1024 / 10;
    match lane {
        Lane::Mac => 97 * GIB_TENTHS,
        Lane::Android => 87 * GIB_TENTHS,
    }
}

/// The disk a new checkout of <lane> adds on a host: `checkout_kib`, less
/// what its clone shares with a source there (`cloned_kib`) when the host has
/// one.
pub fn new_checkout_kib(lane: Lane, has_source: bool) -> u64 {
    if has_source {
        checkout_kib(lane) - cloned_kib(lane)
    } else {
        checkout_kib(lane)
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

/// Adds a checkout of <at>'s lane on a host for a build that joined the queue
/// and waits behind the builds that hold the lane's locks, when a host and the
/// caps allow it, and returns its entry. A host whose
/// free disk would go under <floor_kib> takes none. <now> is the time the
/// checkout's record gives, in seconds since 1970. When builds adds none, it
/// says why.
pub fn grow(at: RepoLane, floor_kib: u64, now: i64) -> Result<Option<String>> {
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
    let refusing: Vec<String> = if crate::app_platform::of(lane).builds_with_xcode() {
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
    let order = queue.order()?;
    // This counts every waiting build, not only this one: of two that wait,
    // the first to add may add the checkout that the queue then gives to the
    // other.
    let slots = set.len();
    if order.len() <= slots {
        return Ok(None);
    }
    let reason = format!(
        "{} for the {} lane's {}",
        crate::text::counted(order.len() - slots, "build waited", "builds waited"),
        lane.word(),
        crate::text::counted(slots, "checkout", "checkouts")
    );
    if set.len() >= LANE_CAP {
        note(format!(
            "{repo}'s {} lane has {LANE_CAP} app checkouts, the most it has, so builds adds none",
            lane.word()
        ));
        return Ok(None);
    }
    let mut cloning = Vec::new();
    for (host, _) in &probes {
        if crate::app_clone::has_source(at, &host.name)? {
            cloning.push(host.name.clone());
        }
    }
    let (fits, mut lacks) = fitting_hosts(lane, floor_kib, &probes, &refusing, &cloning);
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
                at: now,
                why: why.clone(),
            },
        });
        places.write_app_checkouts(repo, lane, &set)?;
        drop(host_guard);
        drop(lane_guard);
        queue::repoint(places, repo, lane)?;
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
/// Macs, are not in <refusing>, have `FREE_CORES` free and would keep their free disk
/// above <floor_kib> with one more checkout of the lane, which takes only
/// what its clone does not share (`new_checkout_kib`) on a host in <cloning>,
/// the hosts with a checkout of the lane to clone. With them, what each other
/// host lacks. The caller checks `HOST_CAP` under the host's guard.
fn fitting_hosts(
    lane: Lane,
    floor_kib: u64,
    probes: &[(Host, std::result::Result<Probe, String>)],
    refusing: &[String],
    cloning: &[String],
) -> (Vec<(Host, Probe)>, Vec<String>) {
    let mut fits: Vec<(Host, Probe)> = Vec::new();
    let mut lacks = Vec::new();
    for (host, probe) in probes {
        if let Some(why) = host::passed_over(host, probe, floor_kib) {
            lacks.push(why.to_string());
            continue;
        }
        let Ok(probe) = probe else {
            unreachable!("passed_over passes over a host that does not answer");
        };
        if probe.platform.os != crate::host_platform::Os::Mac {
            lacks.push(format!(
                "{} runs {}, and app builds run only on a Mac",
                host.name,
                probe.platform.os.word()
            ));
        } else if refusing.contains(&host.name) {
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
        } else if probe.free_kib < floor_kib + new_checkout_kib(lane, cloning.contains(&host.name))
        {
            lacks.push(format!(
                "{} has {} free, and a {} lane checkout that adds about {} would take it under the floor of {}",
                host.name,
                human(probe.free_kib),
                lane.word(),
                human(new_checkout_kib(lane, cloning.contains(&host.name))),
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

/// When a build last used <entry>, a checkout of <at>'s lane: the later of
/// when the last build let its lock go and the times in its last holder's
/// record of its run (for a build that stopped before it could write when it
/// let go). Seconds since 1970; None when neither record exists.
pub fn last_use(at: RepoLane, entry: &str) -> Result<Option<i64>> {
    let RepoLane {
        places, repo, lane, ..
    } = at;
    let mut last: Option<i64> = None;
    let ended = places.app_lock_ended_file(repo, lane, entry);
    match std::fs::read_to_string(&ended) {
        Ok(text) => match text.trim().parse::<i64>() {
            Ok(at) => last = last.max(Some(at)),
            Err(_) => fail!("{} holds no time: {text:?}", ended.display()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => fail!("reading {}: {error}", ended.display()),
    }
    if let Some(run) = HolderRun::read(&places.app_lock_run_file(repo, lane, entry))? {
        last = last.max(Some(run.checkout_at()));
        if let Some(step) = run.step {
            last = last.max(Some(step.started_at));
        }
    }
    Ok(last)
}

/// Removes each checkout of <at>'s lane that builds added and no build has
/// used for `IDLE_MINUTES`, or with <run> dry, says which it would remove. A
/// checkout whose host does not answer, or whose removal there fails, stays
/// in the set, builds prints why, and the next `shrink` removes it. <now> is
/// the time it judges idleness by, in seconds since 1970.
pub fn shrink(at: RepoLane, run: Run, now: i64) -> Result<()> {
    let RepoLane {
        places, repo, lane, ..
    } = at;
    for checkout in places.app_checkouts_recorded(repo, lane)? {
        let AddedBy::Builds { at: added_at, .. } = checkout.added_by else {
            continue;
        };
        let entry = checkout.entry;
        let Some(lock) = lock_checkout(places, repo, lane, &entry)? else {
            // A build holds it.
            continue;
        };
        remove_if_idle(at, &lock, Idle { added_at, now }, run)?;
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

/// `shrink` for the checkout of <at>'s lane whose lock <lock> the caller
/// holds.
fn remove_if_idle(at: RepoLane, lock: &CheckoutLock, times: Idle, run: Run) -> Result<()> {
    let entry = lock.entry.as_str();
    let RepoLane {
        places,
        ssh,
        repo,
        lane,
    } = at;
    let Idle { added_at, now } = times;
    let idle = minutes_since(
        now,
        last_use(at, entry)?.map_or(added_at, |used| used.max(added_at)),
    );
    if idle < IDLE_MINUTES {
        return Ok(());
    }
    let why = format!("no build used it for {idle} minutes");
    let shown = format!("{entry} from {repo}'s {} app checkouts", lane.word());
    if run.dry() {
        note(format!("would remove {shown} ({why})"));
        return Ok(());
    }
    let removed_there = match RemoteApp::find(places, ssh, repo, lane, entry, lock.reservation()) {
        Ok(Some(remote)) => remote.remove(),
        Ok(None) => Err(format!("{entry} is not on a host")),
        Err(error) => Err(error.text()),
    };
    if let Err(said) = removed_there {
        note(format!("kept {shown}, though {why}: {said}"));
        return Ok(());
    }
    let removal = Removed {
        at: now,
        entry: entry.to_string(),
        why: why.clone(),
    };
    take_out(places, repo, lane, lock, Some(removal))?;
    note(format!(
        "removed {shown} and its worktree and build folders on the host: {why}"
    ));
    Ok(())
}

/// The lock of one app checkout, which this process holds until the value
/// is dropped.
pub struct CheckoutLock {
    fd: libc::c_int,
    entry: String,
}

impl std::os::fd::AsFd for CheckoutLock {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        // The descriptor is open until this owner drops.
        unsafe { std::os::fd::BorrowedFd::borrow_raw(self.fd) }
    }
}

impl CheckoutLock {
    pub fn reservation(&self) -> crate::app_lock_guard::CheckoutLock<'_> {
        use std::os::fd::AsFd;
        crate::app_lock_guard::CheckoutLock::new(self.as_fd(), None)
    }
}

impl Drop for CheckoutLock {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

/// Takes the lock of <repo>'s <lane> app checkout <entry>, making the lane's
/// lock folder when it has none; None while a build holds the lock. Fails
/// when, with the lock held, the lane's set has no such checkout: only a
/// holder of the lock takes a checkout out of the set (`take_out`), so it
/// stays in the set while the lock is held.
pub fn lock_checkout(
    places: &Places,
    repo: &str,
    lane: Lane,
    entry: &str,
) -> Result<Option<CheckoutLock>> {
    let dir = places.app_locks_dir(repo, lane);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let Some(fd) = queue::try_lock(&places.app_lock_file(repo, lane, entry))? else {
        return Ok(None);
    };
    let lock = CheckoutLock {
        fd,
        entry: entry.to_string(),
    };
    if !places
        .app_checkouts_of(repo, lane)?
        .iter()
        .any(|e| e == entry)
    {
        fail!(
            "{entry} is not one of {repo}'s {} app checkouts",
            lane.word()
        );
    }
    Ok(Some(lock))
}

/// How `wait_checkout` ended.
pub enum Waited {
    /// The holder let go, and the checkout is still in the lane's set.
    Taken(CheckoutLock),
    /// A takeover or an install stopped this build during the wait.
    Stopped,
    /// The holder removed the checkout from the lane while this waited.
    Gone,
}

/// Waits for the lock of <repo>'s <lane> app checkout <entry> until the build
/// that holds it lets go, then takes it as `lock_checkout` does
/// (lock_wake.rs `wait_for_lock`). A stop signal fails the wait.
pub fn wait_checkout(places: &Places, repo: &str, lane: Lane, entry: &str) -> Result<Waited> {
    let file = places.app_lock_file(repo, lane, entry);
    let waiting_for = format!("{entry}'s lock");
    let Some(fd) =
        crate::lock_wake::wait_for_lock(&file, &waiting_for, &crate::lock::takeover_stopped)?
    else {
        return Ok(Waited::Stopped);
    };
    let lock = CheckoutLock {
        fd,
        entry: entry.to_string(),
    };
    // A holder that removed the checkout deleted its lock file: the lock is
    // on a file no one else can take, and the lane's set no longer has it.
    let _guard = Queue::of(places, repo, lane).guard()?;
    let in_set = places
        .app_checkouts_of(repo, lane)?
        .iter()
        .any(|e| e == entry);
    Ok(if in_set {
        Waited::Taken(lock)
    } else {
        Waited::Gone
    })
}

/// Takes the checkout that <lock> locks out of <repo>'s <lane> set, under the
/// lane's queue guard, and deletes its lock file and the records beside it
/// (its last holder's run and when that holder let it go). With <removal>,
/// builds removed the checkout, which it added on a host: the lane's removal
/// record gets the line, and the folder here that holds the checkout's copied
/// results goes too. Then it tells the builds in the lane's queue to read the
/// set again. <lock> comes from `lock_checkout`, which found the checkout in
/// the set.
pub fn take_out(
    places: &Places,
    repo: &str,
    lane: Lane,
    lock: &CheckoutLock,
    removal: Option<Removed>,
) -> Result<()> {
    let entry = lock.entry.as_str();
    let queue = Queue::of(places, repo, lane);
    let guard = queue.guard()?;
    let mut set = places.app_checkouts_recorded(repo, lane)?;
    set.retain(|checkout| checkout.entry != entry);
    places.write_app_checkouts(repo, lane, &set)?;
    if let Some(removal) = removal {
        let mut removals: Vec<Removed> = places
            .app_checkouts_removed(repo, lane)?
            .into_iter()
            .filter(|kept| kept.at > removal.at - REMOVED_KEPT_SECONDS)
            .collect();
        removals.push(removal);
        let text: String = removals
            .iter()
            .map(|removal| format!("{}\n", removal.line()))
            .collect();
        crate::whole_file::write(&places.app_checkouts_removed_file(repo, lane), text)?;
        let folder = places.root.join(repo).join(entry);
        match std::fs::remove_dir_all(&folder) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("removing {}: {error}", folder.display())
            }
            _ => {}
        }
    }
    for file in [
        places.app_lock_file(repo, lane, entry),
        places.app_lock_run_file(repo, lane, entry),
        places.app_lock_ended_file(repo, lane, entry),
    ] {
        match std::fs::remove_file(&file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("removing {}: {error}", file.display())
            }
            _ => {}
        }
    }
    drop(guard);
    queue::repoint(places, repo, lane)
}

/// `shrink` for every lane of every repo on the volume, for `builds prune`.
pub fn shrink_all(places: &Places, ssh: &Ssh, run: Run, now: i64) -> Result<()> {
    for repo in places.repo_folder_names()? {
        for lane in Lane::ALL {
            let at = RepoLane {
                places,
                ssh,
                repo: &repo,
                lane,
            };
            shrink(at, run, now)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "lane_size_tests.rs"]
mod tests;
