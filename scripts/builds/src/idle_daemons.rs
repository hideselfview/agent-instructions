//! The build daemons that app builds leave running on a host, and which of
//! them `builds health` stops when the host is under memory pressure: the
//! Gradle daemon, the Kotlin compile daemon and the booted simulators. A
//! daemon stays after the build that started it, keeps its memory, and on a
//! 16 GB host a few of them put the whole host under memory pressure, which
//! holds back dispatch for work that only wants a quiet host.
//!
//! A daemon belongs to the lane of the builds that use it (Gradle and Kotlin
//! to the android lane, simulators to the mac lane), and is idle when no build
//! of that lane has used the host for `IDLE_MINUTES` (the time builds already
//! uses to remove a checkout it added). The last build comes from the lane's
//! locks and run records here, which cover every checkout on the host,
//! including one builds has since removed. A lane with a build running on the
//! host keeps its daemons. A host with no build of the lane on record has
//! only the daemon's own age to go by.
//!
//! `judge` reads a `ps` listing and the lane's use, and decides; `stop_on`
//! reads and stops on a real host.

use crate::app_remote::split;
use crate::app_timing;
use crate::fail::Result;
use crate::host::{Host, Ssh};
use crate::host_scan::Row;
use crate::host_scan::parse_ps;
use crate::lane_size::{IDLE_MINUTES, RepoLane, last_use};
use crate::places::Places;
use crate::text::duration;
use boards::app_build::Platform;

/// A daemon using this share of a core or more is working now, whatever the
/// builds record says.
const WORKING_PERCENT: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Gradle,
    KotlinCompile,
    /// A booted simulator device (its `launchd_sim`).
    Simulator,
}

impl Kind {
    /// The platform whose builds use this kind of daemon.
    fn platform(self) -> Platform {
        match self {
            Kind::Gradle | Kind::KotlinCompile => Platform::Android,
            Kind::Simulator => Platform::Mac,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Gradle => "Gradle daemon",
            Kind::KotlinCompile => "Kotlin compile daemon",
            Kind::Simulator => "simulator",
        }
    }

    /// The kind of daemon whose command line is <command>.
    fn of(command: &str) -> Option<Kind> {
        if command.contains("org.gradle.launcher.daemon.bootstrap.GradleDaemon") {
            Some(Kind::Gradle)
        } else if command.contains("org.jetbrains.kotlin.daemon.KotlinCompileDaemon") {
            Some(Kind::KotlinCompile)
        } else if command.contains("/CoreSimulator/Devices/") && command.contains("launchd_sim") {
            Some(Kind::Simulator)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Daemon {
    pub pid: i32,
    pub kind: Kind,
    pub age_seconds: i64,
}

/// What a lane has done on a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneUse {
    /// A build holds the lock of one of the lane's checkouts on the host.
    Building,
    /// The last build of the lane on the host ended at this time (seconds
    /// since 1970); None when no build is on record.
    LastBuild(Option<i64>),
}

/// The daemons among <rows> that user <uid> runs and that do no work now.
pub fn daemons(rows: &[Row], uid: u32) -> Vec<Daemon> {
    rows.iter()
        .filter(|row| {
            row.uid == uid
                && row
                    .cpu_percent
                    .is_some_and(|percent| percent < WORKING_PERCENT)
        })
        .filter_map(|row| {
            Some(Daemon {
                pid: row.pid,
                kind: Kind::of(&row.command)?,
                age_seconds: row.age_seconds,
            })
        })
        .collect()
}

/// A daemon that no build uses, and for how long it has gone unused.
#[derive(Clone, Debug, PartialEq)]
pub struct Idle {
    pub daemon: Daemon,
    /// Seconds since the lane's last build, or since the daemon started when
    /// that is shorter; the daemon's age when no build is on record.
    pub seconds: i64,
    /// Whether a build of the lane is on record.
    pub recorded: bool,
}

/// The daemons of <daemons> that no build uses at <now>, by <use_of>: the
/// lane's use on the host.
pub fn judge(daemons: Vec<Daemon>, use_of: &dyn Fn(Platform) -> LaneUse, now: i64) -> Vec<Idle> {
    daemons
        .into_iter()
        .filter_map(|daemon| {
            let (seconds, recorded) = match use_of(daemon.kind.platform()) {
                LaneUse::Building => return None,
                LaneUse::LastBuild(Some(end)) => ((now - end).max(0).min(daemon.age_seconds), true),
                LaneUse::LastBuild(None) => (daemon.age_seconds, false),
            };
            (seconds / 60 >= IDLE_MINUTES).then_some(Idle {
                daemon,
                seconds,
                recorded,
            })
        })
        .collect()
}

/// How <idle>'s daemon gets stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Kill(i32),
    /// `xcrun simctl shutdown all`: one command stops every booted simulator.
    ShutdownSimulators,
}

impl Stop {
    fn script(self) -> String {
        match self {
            Stop::Kill(pid) => format!("kill {pid}"),
            Stop::ShutdownSimulators => "xcrun simctl shutdown all".to_string(),
        }
    }
}

fn stop_of(daemon: &Daemon) -> Stop {
    match daemon.kind {
        Kind::Simulator => Stop::ShutdownSimulators,
        Kind::Gradle | Kind::KotlinCompile => Stop::Kill(daemon.pid),
    }
}

/// Stops each of <idle> with <run>, at most once for the simulators, and
/// returns one line for each stop that worked and each that failed. <place> is
/// the host as `host::place` says it and <pressure> its memory
/// (`Memory::text`).
pub fn stop(
    idle: &[Idle],
    place: &str,
    pressure: &str,
    run: &dyn Fn(Stop) -> std::result::Result<(), String>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut simulators_done = false;
    for found in idle {
        let kind = found.daemon.kind;
        let action = stop_of(&found.daemon);
        if action == Stop::ShutdownSimulators {
            if simulators_done {
                continue;
            }
            simulators_done = true;
        }
        let used = if found.recorded {
            format!(
                "no {} lane build used {place} for {}",
                kind.platform().word(),
                duration(found.seconds)
            )
        } else {
            format!(
                "no {} lane build is on record for {place}, and it has run {}",
                kind.platform().word(),
                duration(found.seconds)
            )
        };
        lines.push(match run(action) {
            Ok(()) => format!(
                "{place} is under {pressure}: stopped the {} (pid {}) because {used}",
                kind.name(),
                found.daemon.pid
            ),
            Err(said) => format!(
                "{place} is under {pressure}: stopping the {} (pid {}) failed: {said}",
                kind.name(),
                found.daemon.pid
            ),
        });
    }
    lines
}

/// What the lanes on <platform> have done on <host>, from their locks and
/// run records in every repo.
pub fn lane_use(places: &Places, ssh: &Ssh, host: &Host, platform: Platform) -> Result<LaneUse> {
    let on_host = |entry: &str| match split(entry) {
        Some((name, _)) => name == host.name,
        None => host.target.is_none(),
    };
    let mut last: Option<i64> = None;
    for repo in places.repo_folder_names()? {
        let lanes = crate::repo_lanes::with_files(places, &repo)?
            .into_iter()
            .filter(|lane| lane.platform() == platform);
        for lane in lanes {
            for entry in places
                .app_checkouts_of(&repo, lane)?
                .iter()
                .filter(|entry| on_host(entry))
            {
                if crate::flock::is_held(&places.app_lock_file(&repo, lane, entry))? {
                    return Ok(LaneUse::Building);
                }
                let at = RepoLane {
                    places,
                    ssh,
                    repo: &repo,
                    lane,
                };
                last = last.max(last_use(at, entry)?);
            }
            // Every project that holds the repo records its own runs.
            for database in crate::lists::store::databases_holding(places, &repo)? {
                for (entry, end) in app_timing::last_ends(&database, lane)? {
                    if on_host(&entry) {
                        last = last.max(Some(end));
                    }
                }
            }
        }
    }
    Ok(LaneUse::LastBuild(last))
}

fn run_script(ssh: &Ssh, host: &Host, script: &str) -> std::result::Result<String, String> {
    crate::host::output_at(ssh, host, script, &format!("`{script}` on {}", host.name))
}

/// The user id on the first line of <listing> (`LISTING_SCRIPT`'s output) and
/// the rows after it.
pub fn read_listing(listing: &str) -> std::result::Result<(u32, Vec<Row>), String> {
    let (first, rest) = listing.split_once('\n').unwrap_or((listing, ""));
    let uid = first
        .strip_prefix("uid=")
        .and_then(|uid| uid.trim().parse().ok())
        .ok_or_else(|| format!("the process listing starts with {first:?}, not uid=<number>"))?;
    Ok((uid, parse_ps(rest)?))
}

/// The user id and every process, in the form `read_listing` reads.
fn listing_script() -> String {
    format!("echo uid=$(id -u); {}", crate::host_scan::ps_script())
}

/// Stops each daemon on <host> that no build uses, when the host is under
/// <pressure> (`Memory::text`), and returns one line for each. A host with a
/// build running on a lane keeps that lane's daemons.
pub fn stop_on(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    pressure: &str,
    now: i64,
) -> std::result::Result<Vec<String>, String> {
    let (uid, rows) = read_listing(&run_script(ssh, host, &listing_script())?)?;
    let found = daemons(&rows, uid);
    if found.is_empty() {
        return Ok(Vec::new());
    }
    // One reading of each lane, taken after the listing, so a build that
    // started since the listing keeps its daemons.
    let android = lane_use(places, ssh, host, Platform::Android).map_err(|fault| fault.text())?;
    let mac = lane_use(places, ssh, host, Platform::Mac).map_err(|fault| fault.text())?;
    let web = lane_use(places, ssh, host, Platform::Web).map_err(|fault| fault.text())?;
    let use_of = |platform: Platform| match platform {
        Platform::Android => android,
        Platform::Mac => mac,
        Platform::Web => web,
    };
    let idle = judge(found, &use_of, now);
    Ok(stop(
        &idle,
        &crate::host::place(&host.name),
        pressure,
        &|action| run_script(ssh, host, &action.script()).map(|_| ()),
    ))
}

#[cfg(test)]
#[path = "idle_daemons_tests.rs"]
mod tests;
