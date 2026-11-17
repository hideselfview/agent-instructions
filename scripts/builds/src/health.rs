//! `builds health`: what on this machine is stuck or needs acting on, for a
//! coordinator that checks every few minutes. It reads; the one thing it
//! keeps is its own record of sccache's finished compiles, to tell next time
//! whether the server finished any since.
//!
//! It exits 1 when something needs acting on: clients waiting on an sccache
//! server that finished nothing since the last readings, agent shells that are
//! junk (shells.rs), a claimed worktree with no change and no build for an hour
//! (its own, a `builds cargo` run of its own on another host, a `builds
//! app-build` of its branch, or one it holds the app lock for), the volume
//! under its
//! free-space floor, a lists store that fails its check, a running item
//! whose worktree is gone or has no claim (lists/running.rs), a parked item
//! whose worktree is gone (lists/parked.rs), a held worktree
//! whose item waits on nothing open, commits that change design files no
//! recorded design change covers (lists/held.rs), a repo with a design folder
//! and no design globs (lists/project.rs), or a build host that refuses
//! jobs, whose Xcode cannot start, whose installed `builds` differs from this
//! Mac's or whose sccache server launchd does not run (host_installed.rs), or
//! that lacks a setting that UI tests need or whose UI tests timed out turning on
//! automation mode (ui_test_settings.rs), a host whose cores sit idle while
//! items are ready or whose load is well over its cores (host_load.rs), or a
//! project's main checkout whose main lacks commits that origin/main has
//! (main_checkout.rs).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cargo_place::RemoteRun;
use crate::checkout::{Root, parallel_map};
use crate::claim::Claim;
use crate::fail::{Context, Fail, Result};
use crate::holder_run::{HolderRun, RunTimes};
use crate::host::Host;
use crate::lane::Lane;
use crate::places::DiskSpace;
use crate::procs::{
    BUILD_NAMES, BuildProcess, Place, Process, Processes, args, build_processes, cores, live_args,
    load_average, open_files, place_of_process, usage,
};
use crate::shells::Shells;
use crate::side_git::SideGit;
use crate::text::{human, local_minute, minutes_since};
use crate::tool::Tool;

/// An sccache client waiting this long is worth a look.
const CLIENT_MINUTES: i64 = 10;
/// A claimed worktree with no change and no build this long has stalled.
const QUIET_MINUTES: i64 = 60;
/// Readings of sccache's count this old or older say whether it moved.
const READING_MINUTES: i64 = 5;

/// What a finding is about, so the dashboard shows the ones nothing else on
/// its page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingKind {
    Disk,
    Sccache,
    Shells,
    /// A worktree with commits waiting to land and no claim.
    Unclaimed,
    /// A local branch with commits waiting to land that no worktree checks
    /// out and no open item names.
    Branch,
    /// A claimed worktree with no change and no build for a while.
    Quiet,
    Lists,
    /// A worktree whose claim file cannot be read.
    Claim,
    /// A process with a lot of CPU time that looks stuck.
    Busy,
    /// A build host that refuses work, or lacks something builds sets up
    /// there.
    Host,
    /// No dashboard runs.
    Dashboard,
    /// A process whose arguments builds could not read.
    Processes,
    /// A project's main checkout whose main lacks commits that origin/main
    /// has (main_checkout.rs).
    MainCheckout,
    /// A host whose cores sit idle while items are ready, or whose load is
    /// well over its cores (host_load.rs).
    Load,
}

/// What a finding says, and the command it suggests running, if any. The
/// text names no command: a text that suggests one ends with the words that
/// lead into it ("stop it with"), and `line` puts the command after them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advice {
    pub text: String,
    pub command: Option<String>,
}

impl Advice {
    /// A finding that suggests no command.
    pub fn said(text: String) -> Advice {
        Advice {
            text,
            command: None,
        }
    }

    /// A finding that suggests <command>.
    pub fn run(text: String, command: String) -> Advice {
        Advice {
            text,
            command: Some(command),
        }
    }

    /// The text followed by its command, as `builds health` prints it.
    pub fn line(&self) -> String {
        match &self.command {
            Some(command) => format!("{} `{command}`", self.text),
            None => self.text.clone(),
        }
    }
}

/// One line of what needs acting on: what it is about, and what it says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    #[serde(flatten)]
    pub advice: Advice,
}

impl Finding {
    pub fn new(kind: FindingKind, advice: Advice) -> Finding {
        Finding { kind, advice }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Health {
    pub generated_at: i64,
    pub load: [f64; 3],
    pub cores: usize,
    #[serde(flatten)]
    pub disk: DiskSpace,
    pub sccache: SccacheHealth,
    pub shells: Shells,
    pub worktrees: Vec<WorktreeHealth>,
    pub locks: Vec<LockHealth>,
    /// Each lane's one queue.
    pub queues: Vec<LaneQueue>,
    /// What needs acting on, one line each; empty when nothing does.
    pub findings: Vec<Finding>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SccacheHealth {
    pub listening: bool,
    /// "Compile requests executed" now.
    pub executed: Option<u64>,
    /// The reading compared against: the newest one at least
    /// READING_MINUTES old.
    pub compared: Option<Reading>,
    /// Clients waiting CLIENT_MINUTES or more.
    pub waiting: Vec<Client>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Reading {
    pub executed: u64,
    pub at: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Client {
    pub pid: i32,
    pub minutes: i64,
    pub cpu_seconds: f64,
    pub checkout: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WorktreeHealth {
    pub checkout: String,
    pub path: String,
    pub claim: Claim,
    pub branch: Option<String>,
    pub git: SideGit,
    /// Minutes since the newer of HEAD's commit and the newest uncommitted
    /// write.
    pub quiet_minutes: Option<i64>,
    /// No change and no build for QUIET_MINUTES, and not held or waiting on
    /// the person's review: health lists it under needs action, and the
    /// dashboard's card says it stalled.
    pub stalled: bool,
    pub building: Vec<BuildProcess>,
    /// Its `builds cargo` runs whose cargo runs on another host.
    pub remote_runs: Vec<RemoteRun>,
    /// The `builds app-build` requests that build for it
    /// (`AppBuildRun::builds_for`).
    pub app_builds: Vec<AppBuildRun>,
    /// The holder of an app lock building for this worktree in the app
    /// checkout (`Holder::builds_for`).
    pub app_build: Option<i32>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LockHealth {
    pub repo: String,
    /// The app lane the lock is for (`mac`, `android`).
    pub lane: String,
    /// The app checkout (side-N) the lock is for.
    pub checkout: String,
    pub path: String,
    pub holder: Option<Holder>,
}

/// The one queue of a repo's lane: those waiting for whichever of the lane's
/// locks frees first, next first.
#[derive(Clone, Serialize, Deserialize)]
pub struct LaneQueue {
    pub repo: String,
    pub lane: String,
    pub queue: Vec<Queued>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Queued {
    pub pid: i32,
    pub minutes: i64,
    pub command: String,
    /// The checkout its working directory is in.
    pub started_from: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Holder {
    pub pid: i32,
    pub minutes: i64,
    pub command: String,
    /// What it runs: its children's commands.
    pub running: Vec<String>,
    /// The checkout its working directory is in: for `builds lock`, the
    /// worktree it was run from.
    pub started_from: Option<String>,
    /// The app checkout's HEAD while it holds the lock: the commit it builds.
    pub app_head: Option<String>,
    /// Does the `builds lock`, `build-app` or `app-build` process that took
    /// the lock still run? False when only processes it started still have
    /// the lock file open: the build is over and what is left keeps the lock.
    pub driver_running: bool,
    /// How long the build waited for the lock, how long it has built, and
    /// the step it runs, from the record it keeps beside the lock
    /// (holder_run.rs); None when the process that took the lock has kept no
    /// record or no longer runs.
    pub run: Option<RunTimes>,
}

/// A `builds app-build` request that runs: the process that the person or
/// agent started. Each lane's process waits in that lane's queue and then
/// builds in an app checkout here or on another host, and the request waits
/// until every lane's process ends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppBuildRun {
    pub pid: i32,
    pub repo: String,
    /// The ref the request builds: the branch (or commit) that whoever ran
    /// `builds app-build` gave it.
    pub reference: String,
    /// The checkout its working directory is in.
    pub started_from: Option<String>,
}

impl AppBuildRun {
    /// Does it build for the worktree <label> of <repo>, which has <branch>
    /// checked out? When it builds that branch, or it was run from there.
    fn builds_for(&self, label: &str, repo: &str, branch: Option<&str>) -> bool {
        self.started_from.as_deref() == Some(label)
            || (self.repo == repo && Some(self.reference.as_str()) == branch)
    }
}

impl Holder {
    /// Does it build for the worktree <label> at <git>? When it was run from
    /// there, or the app checkout is at a commit of that worktree's own (one
    /// origin/main has no copy of), as when it was run from elsewhere or its
    /// `builds lock` was killed and a process of its build holds the lock.
    fn builds_for(&self, label: &str, git: &SideGit) -> bool {
        self.started_from.as_deref() == Some(label)
            || (self.app_head.is_some()
                && git
                    .ahead
                    .iter()
                    .any(|c| Some(&c.full) == self.app_head.as_ref()))
    }
}

/// A repo's app lock for one of a lane's app checkouts, and that checkout.
pub struct AppLock {
    pub repo: String,
    pub lane: Lane,
    /// The app checkout (side-N) the lock is for.
    pub checkout: String,
    pub path: PathBuf,
    /// The record its holder keeps of its run (holder_run.rs).
    pub run_file: PathBuf,
    /// The app checkout's top, when it is a live checkout.
    pub app_top: Option<PathBuf>,
}

/// The record of sccache readings between runs.
fn readings_file(home: &Path) -> PathBuf {
    home.join("Library/Caches/builds/sccache-readings.json")
}

fn read_readings(file: &Path) -> Result<Vec<Reading>> {
    match std::fs::read(file) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).context(|| format!("reading {}", file.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(Fail::Message(format!(
            "reading {}: {error}",
            file.display()
        ))),
    }
}

fn write_readings(file: &Path, readings: &[Reading]) -> Result<()> {
    let dir = file.parent().expect("the file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let staged = file.with_extension(crate::text::staged_suffix());
    std::fs::write(
        &staged,
        serde_json::to_vec(readings).expect("readings serialize"),
    )
    .context(|| format!("writing {}", staged.display()))?;
    std::fs::rename(&staged, file).context(|| format!("writing {}", file.display()))
}

fn command_line(pid: i32) -> String {
    let line = args(pid).unwrap_or_default().join(" ");
    if line.chars().count() > 200 {
        format!("{}…", line.chars().take(200).collect::<String>())
    } else {
        line
    }
}

/// What the processes and sccache say now: the part of health a daemon
/// samples while a page is open, and a command reads once.
#[derive(Clone, Serialize, Deserialize)]
pub struct Machine {
    pub at: i64,
    pub load: [f64; 3],
    pub cores: usize,
    /// Build processes, each placed in one of the places it was sampled
    /// with.
    pub builds: Vec<BuildProcess>,
    /// `builds cargo` runs that use another host, each placed in the place
    /// of its `builds cargo` process.
    pub remote_runs: Vec<RemoteRun>,
    /// The `builds app-build` requests that run.
    pub app_builds: Vec<AppBuildRun>,
    /// What builds could not read of the processes it needed, one line each.
    pub unread: Vec<String>,
    /// The `git commit`s that run, which keep their worktree from being free.
    pub commits: crate::procs::GitCommits,
    pub sccache: SccacheHealth,
    pub shells: Shells,
    pub locks: Vec<LockHealth>,
    pub queues: Vec<LaneQueue>,
}

/// sccache's count of finished compiles now, against the newest reading at
/// least READING_MINUTES old; this reading joins the record when the newest
/// in it is a minute old or more.
pub fn sccache_reading(home: &Path, now: i64) -> Result<(bool, Option<u64>, Option<Reading>)> {
    let listening = crate::sccache::listening()?;
    let executed = if listening {
        crate::sccache::count(&crate::sccache::stats(), "Compile requests executed")
    } else {
        None
    };
    let file = readings_file(home);
    let mut readings = read_readings(&file)?;
    let compared = readings
        .iter()
        .filter(|r| now - r.at >= READING_MINUTES * 60)
        .max_by_key(|r| r.at)
        .cloned();
    if let Some(executed) = executed
        && readings.iter().all(|r| now - r.at >= 60)
    {
        readings.push(Reading { executed, at: now });
        // Keep what the next runs compare against: the last hour.
        readings.retain(|r| now - r.at <= 3600);
        write_readings(&file, &readings)?;
    }
    Ok((listening, executed, compared))
}

/// Reads the processes now: build processes placed in <places>, the
/// `builds cargo` runs there that use one of <hosts>, the `builds app-build`
/// requests, sccache's waiting clients, agent shells, and the holders of
/// <locks> (each repo's app.lock).
/// <sccache> is a `sccache_reading`.
pub fn sample(
    places: &[Place],
    hosts: &[Host],
    locks: &[AppLock],
    uid: u32,
    home: &Path,
    now: i64,
    sccache: (bool, Option<u64>, Option<Reading>),
) -> Machine {
    let processes = Processes::read();
    let builds = build_processes(&processes, places, &BUILD_NAMES);
    let (remote_runs, mut unread) = crate::cargo_place::remote_runs(&processes, places, hosts);
    let app_builds = processes
        .list
        .iter()
        .filter(|process| process.name == "builds")
        .filter_map(|process| {
            let argv = match live_args(process.pid) {
                Ok(argv) => argv?,
                Err(error) => {
                    unread.push(format!(
                        "{error}, so health cannot tell whether it is a `builds app-build` request"
                    ));
                    return None;
                }
            };
            let (repo, reference) = crate::app_build::requested(&argv)?;
            Some(AppBuildRun {
                pid: process.pid,
                repo: repo.to_string(),
                reference: reference.to_string(),
                started_from: started_from(places, process.pid),
            })
        })
        .collect();
    let (listening, executed, compared) = sccache;
    let mut waiting = Vec::new();
    for process in &processes.list {
        if process.name != "sccache" || minutes_since(now, process.started) < CLIENT_MINUTES {
            continue;
        }
        // The server runs with no arguments; a client names its compiler.
        if args(process.pid).is_none_or(|argv| argv.len() < 2) {
            continue;
        }
        let checkout = builds
            .iter()
            .find(|b| b.pid == process.pid)
            .map(|b| places[b.place].label.clone());
        waiting.push(Client {
            pid: process.pid,
            minutes: minutes_since(now, process.started),
            cpu_seconds: usage(process.pid).map(|(cpu, _)| cpu).unwrap_or(0.0),
            checkout,
        });
    }
    waiting.sort_by_key(|c| std::cmp::Reverse(c.minutes));
    let mut held = Vec::new();
    for AppLock {
        repo,
        lane,
        checkout,
        path,
        run_file,
        app_top,
    } in locks
    {
        let shown = path.display().to_string();
        let real = std::fs::canonicalize(path).unwrap_or(path.clone());
        let opened = |pid: i32| {
            open_files(pid)
                .iter()
                .any(|open| *open == real || open == path)
        };
        // `builds lock <repo> ...`, `builds build-app <repo> ...` and
        // `builds app-build <repo> ...`: the one with the lock's file open
        // holds it; the others wait in its queue.
        let mut holding = processes
            .list
            .iter()
            .filter(|p| p.name == "builds")
            .find(|process| {
                let locking = |argv: Vec<String>| {
                    argv.get(1)
                        .is_some_and(|a| a == "lock" || a == "build-app" || a == "app-build")
                        && argv.get(2) == Some(repo)
                };
                args(process.pid).is_some_and(locking) && opened(process.pid)
            });
        // A build whose `builds lock` was killed outright still holds the
        // lock: its processes have the file open. Found only when the lock is
        // taken, since reading every process's open files costs.
        let driver_running = holding.is_some();
        if holding.is_none() && taken(path) {
            holding = topmost(&processes, uid, opened);
        }
        let run = match holding.filter(|_| driver_running) {
            None => None,
            Some(process) => match HolderRun::read(run_file) {
                Ok(record) => record
                    .filter(|record| record.pid == process.pid)
                    .map(|record| record.times(now)),
                Err(error) => {
                    unread.push(error.text());
                    None
                }
            },
        };
        let holder = holding.map(|process| Holder {
            pid: process.pid,
            minutes: minutes_since(now, process.started),
            command: command_line(process.pid),
            running: processes
                .children(process.pid)
                .map(|child| command_line(child.pid))
                .collect(),
            started_from: started_from(places, process.pid),
            app_head: app_top.as_deref().and_then(head_of),
            driver_running,
            run,
        });
        held.push(LockHealth {
            repo: repo.clone(),
            lane: lane.word().to_string(),
            checkout: checkout.clone(),
            path: shown,
            holder,
        });
    }
    let mut queues: Vec<LaneQueue> = Vec::new();
    for AppLock {
        repo, lane, path, ..
    } in locks
    {
        if queues
            .iter()
            .any(|q| q.repo == *repo && q.lane == lane.word())
        {
            continue;
        }
        // `Queue::waiting` is those after the lane's holders.
        let in_line = path
            .parent()
            .and_then(|locks_dir| locks_dir.parent())
            .map(|repo_dir| crate::queue::Queue::in_repo_dir(repo_dir, *lane).waiting())
            .unwrap_or_default();
        let queue = in_line
            .into_iter()
            .filter_map(|pid| {
                processes
                    .list
                    .iter()
                    .find(|p| p.pid == pid && p.name == "builds")
            })
            .map(|process| Queued {
                pid: process.pid,
                minutes: minutes_since(now, process.started),
                command: command_line(process.pid),
                started_from: started_from(places, process.pid),
            })
            .collect();
        queues.push(LaneQueue {
            repo: repo.clone(),
            lane: lane.word().to_string(),
            queue,
        });
    }
    let queued: std::collections::HashSet<i32> = queues
        .iter()
        .flat_map(|q| q.queue.iter().map(|w| w.pid))
        .collect();
    let shells = crate::shells::judge(
        &crate::shells::gather(&processes, uid),
        now,
        home,
        places,
        &queued,
    );
    Machine {
        at: now,
        load: load_average(),
        cores: cores(),
        builds,
        remote_runs,
        app_builds,
        unread,
        commits: crate::procs::git_commits(&processes),
        sccache: SccacheHealth {
            listening,
            executed,
            compared,
            waiting,
        },
        shells,
        locks: held,
        queues,
    }
}

/// The label of the checkout the working directory of process <pid> is in.
fn started_from(places: &[Place], pid: i32) -> Option<String> {
    place_of_process(places, pid).map(|at| places[at].label.clone())
}

/// Is the lock at <path> taken? Asked by taking it without waiting and
/// letting it go at once.
fn taken(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_EXLOCK | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd >= 0 {
        unsafe { libc::close(fd) };
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

/// Of <uid>'s processes for which <holds> is true, the oldest one whose
/// parent doesn't hold it too: the top of the build holding a lock.
fn topmost(processes: &Processes, uid: u32, holds: impl Fn(i32) -> bool) -> Option<&Process> {
    let holding: Vec<&Process> = processes
        .list
        .iter()
        .filter(|p| p.uid == uid && holds(p.pid))
        .collect();
    holding
        .iter()
        .filter(|p| !holding.iter().any(|q| q.pid == p.ppid))
        .min_by_key(|p| p.started)
        .copied()
}

/// The full sha of <top>'s HEAD.
fn head_of(top: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["--no-optional-locks", "rev-parse", "HEAD"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !sha.is_empty()).then_some(sha)
}

/// A claimed side worktree as health judges it.
pub struct Claimed {
    /// Its index among the places the machine was sampled with.
    pub place: usize,
    pub claim: Claim,
    pub branch: Option<String>,
    pub git: SideGit,
    /// Do its commits wait on the person's review of a design change?
    pub awaiting_review: bool,
}

/// A side worktree with no claim that holds commits missing from origin/main:
/// work an agent left behind. Never the app checkout, which no one claims:
/// what it has checked out is the branch an app build is building, and never
/// a worktree an open item is parked in, whose work waits there for the item
/// (lists/parked.rs).
pub struct Unclaimed {
    pub place: usize,
    pub git: SideGit,
    /// Why builds cannot tell whether an item is parked there: a store that
    /// does not load. None when builds read every store.
    pub parked_unknown: Option<String>,
}

/// Work an agent left where no claim shows it: side worktrees with no claim
/// that hold commits origin/main lacks, and local branches with such commits
/// that no worktree checks out and no open item names
/// (`unlanded::loose_branches`).
#[derive(Default)]
pub struct LeftBehind {
    pub worktrees: Vec<Unclaimed>,
    pub branches: Vec<crate::unlanded::LooseBranch>,
}

/// Judges what needs acting on.
pub fn assess(
    now: i64,
    disk: DiskSpace,
    machine: Machine,
    places: &[Place],
    claimed: Vec<Claimed>,
    left: LeftBehind,
    lists: Vec<Advice>,
) -> Health {
    let mut findings = Vec::new();
    if disk.free_kib < disk.floor_kib {
        findings.push(Finding::new(
            FindingKind::Disk,
            Advice::said(format!(
                "disk: {} free, under the {} floor",
                human(disk.free_kib),
                human(disk.floor_kib)
            )),
        ));
    }
    let s = &machine.sccache;
    if let Some(oldest) = s.waiting.first().map(|c| c.minutes) {
        match (s.listening, s.executed, &s.compared) {
            (false, _, _) => findings.push(Finding::new(
                FindingKind::Sccache,
                Advice::said(format!(
                    "sccache: no server is listening and {} clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m)",
                    s.waiting.len()
                )),
            )),
            (true, Some(now_count), Some(before)) if now_count == before.executed => findings.push(Finding::new(
                FindingKind::Sccache,
                Advice::said(format!(
                    "sccache: {} clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m) and the server finished no compile since {} ({}m ago)",
                    s.waiting.len(),
                    local_minute(before.at),
                    minutes_since(now, before.at)
                )),
            )),
            _ => {}
        }
    }
    findings.extend(machine.unread.iter().map(|line| {
        Finding::new(
            FindingKind::Processes,
            Advice::said(format!("processes: {line}")),
        )
    }));
    if !machine.shells.junk.is_empty() {
        findings.push(Finding::new(
            FindingKind::Shells,
            Advice::run(
                format!(
                    "shells: {}, listed under shells; stop them with",
                    crate::text::counted(
                        machine.shells.junk.len(),
                        "junk shell runs",
                        "junk shells run"
                    )
                ),
                "builds health --stop-junk".to_string(),
            ),
        ));
    }
    for Unclaimed {
        place,
        git,
        parked_unknown,
    } in left.worktrees
    {
        if !git.ahead.is_empty() {
            let unknown = match parked_unknown {
                Some(fault) => {
                    format!("; builds cannot tell whether an item is parked there, since {fault}")
                }
                None => String::new(),
            };
            findings.push(Finding::new(
                FindingKind::Unclaimed,
                Advice::said(format!(
                    "worktree: {} has no claim and {} waiting to land, so it is not retired{unknown}",
                    places[place].label,
                    crate::text::counted(git.ahead.len(), "commit", "commits")
                )),
            ));
        }
    }
    for branch in left.branches {
        findings.push(Finding::new(FindingKind::Branch, loose_advice(&branch)));
    }
    let mut worktrees = Vec::new();
    for Claimed {
        place,
        claim,
        branch,
        git,
        awaiting_review,
    } in claimed
    {
        let building: Vec<BuildProcess> = machine
            .builds
            .iter()
            .filter(|b| b.place == place)
            .cloned()
            .collect();
        let remote_runs: Vec<RemoteRun> = machine
            .remote_runs
            .iter()
            .filter(|run| run.place == place)
            .cloned()
            .collect();
        let label = places[place].label.clone();
        let app_builds: Vec<AppBuildRun> = machine
            .app_builds
            .iter()
            .filter(|run| run.builds_for(&label, &places[place].repo, branch.as_deref()))
            .cloned()
            .collect();
        let quiet_minutes = git.activity().map(|when| minutes_since(now, when));
        // A worktree claimed a few minutes ago is not idle, even when its
        // files have not changed for hours.
        let idle_minutes = git
            .activity()
            .max(Some(claim.since))
            .map(|when| minutes_since(now, when));
        let app_build = machine
            .locks
            .iter()
            .filter_map(|lock| lock.holder.as_ref())
            .find(|h| h.builds_for(&label, &git))
            .map(|h| h.pid);
        // A held claim is quiet on purpose: its note says why. A worktree
        // whose design change waits on the person's review is waiting on them.
        let held = claim.note_state.as_deref() == Some("held");
        let stalled = !held
            && !awaiting_review
            && building.is_empty()
            && remote_runs.is_empty()
            && app_builds.is_empty()
            && app_build.is_none()
            && idle_minutes.is_none_or(|m| m >= QUIET_MINUTES);
        if stalled {
            findings.push(Finding::new(
                FindingKind::Quiet,
                Advice::said(format!(
                    "worktree: {label} ({}) has had no change for {} and no build runs there",
                    claim.who,
                    idle_minutes
                        .map(|m| format!("{m}m"))
                        .unwrap_or_else(|| "as long as git can tell".to_string())
                )),
            ));
        }
        worktrees.push(WorktreeHealth {
            checkout: label,
            path: places[place].top.display().to_string(),
            claim,
            branch,
            git,
            quiet_minutes,
            stalled,
            building,
            remote_runs,
            app_builds,
            app_build,
        });
    }
    findings.extend(
        lists
            .into_iter()
            .map(|advice| Finding::new(FindingKind::Lists, advice)),
    );
    Health {
        generated_at: now,
        load: machine.load,
        cores: machine.cores,
        disk,
        sccache: machine.sccache,
        shells: machine.shells,
        worktrees,
        locks: machine.locks,
        queues: machine.queues,
        findings,
    }
}

/// What health says of <branch>: its commits, and the command that takes it
/// back into a worktree.
fn loose_advice(branch: &crate::unlanded::LooseBranch) -> Advice {
    Advice::run(
        format!(
            "branch: {} {} holds {} origin/main lacks ({}), and no worktree checks it out or open item names it; check it out in a worktree with",
            branch.repo,
            branch.branch,
            crate::text::counted(branch.commits.len(), "commit", "commits"),
            crate::unlanded::commit_words(&branch.commits)
        ),
        format!(
            "builds new {} --resume {}",
            branch.main.display(),
            branch.branch
        ),
    )
}

/// The finding when health could not read the branches of the repo whose
/// main checkout is <main>.
pub fn branches_unread(main: &Path, error: &str) -> Finding {
    Finding::new(
        FindingKind::Branch,
        Advice::said(format!(
            "branch: health could not read the branches of {}: {error}",
            main.display()
        )),
    )
}

/// The main checkout of each repo among <roots> that has a live one.
pub fn mains_of(roots: &[Root]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter_map(|root| root.live.as_ref())
        .filter(|checkout| checkout.is_main())
        .map(|checkout| checkout.top.clone())
        .collect()
}

/// The places processes are placed in: every live checkout.
pub fn places_of(roots: &[Root]) -> Vec<Place> {
    roots
        .iter()
        .filter(|root| root.live.is_some())
        .map(|root| Place {
            label: format!("{}/{}", root_repo(root), root.name()),
            repo: root_repo(root),
            top: root.top.clone(),
            root: root.path.clone(),
        })
        .collect()
}

/// Each repo's app lock that exists, a lane's each, with the lane's app
/// checkout.
pub fn locks_of(places: &crate::places::Places, roots: &[Root]) -> Result<Vec<AppLock>> {
    let mut repos: Vec<String> = roots.iter().map(root_repo).collect();
    repos.dedup();
    let mut locks = Vec::new();
    for repo in repos {
        for lane in Lane::ALL {
            for app in places.app_checkouts_of(&repo, lane)? {
                let path = places.app_lock_file(&repo, lane, &app);
                if !path.is_file() {
                    continue;
                }
                let app_top = roots
                    .iter()
                    .find(|root| {
                        root_repo(root) == repo && root.name() == app && root.live.is_some()
                    })
                    .map(|root| root.top.clone());
                locks.push(AppLock {
                    repo: repo.clone(),
                    lane,
                    run_file: places.app_lock_run_file(&repo, lane, &app),
                    checkout: app,
                    path,
                    app_top,
                });
            }
        }
    }
    Ok(locks)
}

/// How long <holder> has held its lock, in words: its wait, its building
/// time and its step from the record it keeps, else how long ago its process
/// started.
fn held_for(holder: &Holder) -> String {
    let Some(run) = &holder.run else {
        return format!("started {}m ago", holder.minutes);
    };
    let step = match &run.step {
        Some(step) => format!(", step {} for {}m", step.name, step.minutes),
        None => String::new(),
    };
    format!(
        "waited {}m, building {}m{step}",
        run.waited_minutes, run.building_minutes
    )
}

/// What the lists say, one line each: the running-item and held-work
/// findings about the claimed worktrees (lists/running.rs, lists/held.rs),
/// each parked item whose worktree is gone (lists/parked.rs), and each repo with a design folder and no design globs
/// (lists/project.rs). Also
/// marks the worktrees whose design change waits on the person's review, so
/// <claimed> is judged with that known.
pub fn lists_findings(
    places_in: &crate::places::Places,
    loaded: &std::result::Result<Vec<crate::lists::store::Loaded>, String>,
    places: &[Place],
    claimed: &mut [Claimed],
    worktrees: &[crate::lists::held::Worktree],
) -> Result<Vec<Advice>> {
    let reviewing = crate::lists::held::awaiting_review(loaded, worktrees);
    for one in claimed.iter_mut() {
        one.awaiting_review = reviewing.contains(&places[one.place].label);
    }
    let mut lists = crate::lists::running::findings(places_in, loaded)?;
    lists.extend(crate::lists::parked::findings(loaded)?);
    lists.extend(crate::lists::held::findings(loaded, worktrees));
    lists.extend(crate::lists::project::missing_globs_findings(
        &places_in.home,
        loaded,
    ));
    Ok(lists)
}

/// Gathers the machine's health now, reading everything itself.
pub fn health(tool: &Tool) -> Result<Health> {
    let now = tool.now;
    let roots = tool.roots()?;
    let live: Vec<&Root> = roots.iter().filter(|root| root.live.is_some()).collect();
    let places = places_of(&roots);
    let sccache = sccache_reading(&tool.places.home, now)?;
    let machine = sample(
        &places,
        &crate::host::all(&tool.places)?,
        &locks_of(&tool.places, &roots)?,
        tool.places.uid,
        &tool.places.home,
        now,
        sccache,
    );
    let mut claimed = Vec::new();
    let mut unclaimed = Vec::new();
    let mut unreadable = Vec::new();
    let loaded = crate::lists::store::load_all(&tool.places);
    // While a store does not load, health names each unclaimed worktree
    // with commits and says it cannot tell whether an item is parked there.
    let parked = crate::lists::parked::parked_worktrees(&loaded);
    for (at, root) in live.iter().enumerate() {
        let checkout = root.live.as_ref().expect("live");
        if !checkout.is_side() {
            continue;
        }
        match Claim::read(&checkout.claim_file()) {
            Ok(Some(claim)) => claimed.push((at, checkout, claim)),
            Ok(None) if root.is_app_checkout(&tool.places)? => {}
            Ok(None)
                if parked
                    .as_ref()
                    .is_ok_and(|parked| parked.contains(&checkout.top.display().to_string())) => {}
            Ok(None) => unclaimed.push((at, checkout)),
            Err(damaged) => unreadable.push(Finding::new(
                FindingKind::Claim,
                Advice::said(format!("worktree: {}", damaged.text())),
            )),
        }
    }
    let unclaimed_gits = parallel_map(&unclaimed, |(_, checkout)| {
        crate::side_git::read(&checkout.top)
    });
    let unclaimed: Vec<Unclaimed> = unclaimed
        .into_iter()
        .zip(unclaimed_gits)
        .map(|((place, _), git)| Unclaimed {
            place,
            git,
            parked_unknown: parked.as_ref().err().cloned(),
        })
        .collect();
    let gits = parallel_map(&claimed, |(_, checkout, _)| {
        crate::side_git::read(&checkout.top)
    });
    let mut judged = Vec::new();
    let mut held_worktrees = Vec::new();
    for ((place, checkout, claim), git) in claimed.into_iter().zip(gits) {
        held_worktrees.push(crate::lists::held::Worktree::read(
            &tool.places,
            places[place].label.clone(),
            checkout,
            &claim,
            &git,
            &machine.commits,
        )?);
        judged.push(Claimed {
            place,
            claim,
            branch: checkout.branch()?,
            git,
            awaiting_review: false,
        });
    }
    let lists = lists_findings(&tool.places, &loaded, &places, &mut judged, &held_worktrees)?;
    let mut loose = Vec::new();
    for main in mains_of(&roots) {
        match crate::unlanded::loose_branches(&main, &mut crate::unlanded::Judged::default()) {
            Ok(found) => loose.extend(found),
            Err(error) => unreadable.push(branches_unread(&main, &error.text())),
        }
    }
    let mut health = assess(
        now,
        tool.disk_space()?,
        machine,
        &places,
        judged,
        LeftBehind {
            worktrees: unclaimed,
            branches: crate::unlanded::unheld(loose, &loaded),
        },
        lists,
    );
    health.findings.extend(unreadable);
    health
        .findings
        .extend(crate::main_checkout::findings(&tool.places.home, &loaded));
    Ok(health)
}

fn root_repo(root: &Root) -> String {
    root.path
        .parent()
        .map(crate::checkout::file_name)
        .unwrap_or_default()
}

/// What the build processes of a worktree are: `cargo, rustc x4`.
fn process_summary(building: &[BuildProcess]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for process in building {
        match counts.iter_mut().find(|(name, _)| *name == process.name) {
            Some(entry) => entry.1 += 1,
            None => counts.push((process.name.clone(), 1)),
        }
    }
    counts
        .into_iter()
        .map(|(name, n)| if n == 1 { name } else { format!("{name} x{n}") })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What builds for worktree <w>: its build processes here, its `builds
/// cargo` runs on other hosts, the `builds app-build` requests for it, and
/// the app lock's holder building for it.
fn builds_line(w: &WorktreeHealth) -> String {
    let mut parts = Vec::new();
    if !w.building.is_empty() {
        parts.push(format!("building: {}", process_summary(&w.building)));
    }
    for run in &w.remote_runs {
        parts.push(format!(
            "running cargo on host {} (builds cargo, pid {})",
            run.host, run.pid
        ));
    }
    for run in &w.app_builds {
        parts.push(format!(
            "builds app-build of {} (pid {})",
            run.reference, run.pid
        ));
    }
    if let Some(pid) = w.app_build {
        parts.push(format!("building in the app checkout (pid {pid})"));
    }
    if parts.is_empty() {
        "no build running".to_string()
    } else {
        parts.join("; ")
    }
}

/// The lines `builds health` prints.
pub fn lines(health: &Health) -> Vec<String> {
    let mut out = Vec::new();
    let [one, five, fifteen] = health.load;
    out.push(format!(
        "load: {one:.1} {five:.1} {fifteen:.1} on {} cores",
        health.cores
    ));
    out.push(format!(
        "disk: {} free; warns under {}, stops under {}",
        human(health.disk.free_kib),
        human(health.disk.floor_kib),
        human(health.disk.hard_floor_kib)
    ));
    let s = &health.sccache;
    let mut line = match (s.listening, s.executed) {
        (false, _) => "sccache: no server listening".to_string(),
        (true, None) => "sccache: the server did not say how many compiles it executed".to_string(),
        (true, Some(executed)) => match &s.compared {
            Some(before) => format!(
                "sccache: {executed} compiles executed, {} since {} ({}m ago)",
                executed.saturating_sub(before.executed),
                local_minute(before.at),
                minutes_since(health.generated_at, before.at)
            ),
            None => format!(
                "sccache: {executed} compiles executed; no reading {READING_MINUTES}+ min old to compare with yet"
            ),
        },
    };
    if s.waiting.is_empty() {
        line += &format!("; no client waiting {CLIENT_MINUTES}+ min");
    } else {
        line += &format!(
            "; {} clients waiting {CLIENT_MINUTES}+ min",
            s.waiting.len()
        );
    }
    out.push(line);
    for client in &s.waiting {
        out.push(format!(
            "    pid {} waiting {}m, {:.1}s CPU{}",
            client.pid,
            client.minutes,
            client.cpu_seconds,
            client
                .checkout
                .as_ref()
                .map(|c| format!(", {c}"))
                .unwrap_or_default()
        ));
    }
    out.extend(crate::shells::lines(&health.shells));
    out.push(format!("claimed worktrees: {}", health.worktrees.len()));
    for w in &health.worktrees {
        let git = match &w.git.error {
            Some(error) => format!("git: {error}"),
            None => format!(
                "{} uncommitted, {} waiting to land",
                w.git.uncommitted.unwrap_or(0),
                w.git.ahead.len()
            ),
        };
        out.push(format!(
            "    {}  [{}]  {} since {}{}",
            w.checkout,
            w.branch.as_deref().unwrap_or("detached"),
            w.claim.who,
            local_minute(w.claim.since),
            match (&w.claim.note_state, &w.claim.note) {
                (Some(state), note) => format!(
                    " ({}): {}",
                    state.replace('-', " "),
                    note.as_deref().unwrap_or("")
                ),
                (None, Some(note)) => format!(": {note}"),
                (None, None) => String::new(),
            }
        ));
        out.push(format!(
            "        {git}; last change {}; {}",
            w.quiet_minutes
                .map(|m| format!("{m}m ago"))
                .unwrap_or_else(|| "unknown".to_string()),
            builds_line(w)
        ));
    }
    for lock in &health.locks {
        match &lock.holder {
            Some(holder) => out.push(format!(
                "app lock {} ({} {}): held by pid {} ({}), {}{}",
                lock.repo,
                lock.lane,
                lock.checkout,
                holder.pid,
                holder.command,
                held_for(holder),
                if holder.running.is_empty() {
                    String::new()
                } else {
                    format!("; running {}", holder.running.join("; "))
                }
            )),
            None => out.push(format!(
                "app lock {} ({} {}): free",
                lock.repo, lock.lane, lock.checkout
            )),
        }
    }
    for q in &health.queues {
        out.push(format!(
            "app queue {} ({}): {} waiting",
            q.repo,
            q.lane,
            q.queue.len()
        ));
    }
    if health.findings.is_empty() {
        out.push("needs action: nothing".to_string());
    } else {
        out.push("needs action:".to_string());
        for finding in &health.findings {
            out.push(format!("    {}", finding.advice.line()));
        }
    }
    out
}

/// The findings that come from the hosts: each process that looks stuck on
/// one (stuck.rs), each host that refuses jobs or whose Xcode cannot start,
/// what `host_installed::installed_findings` reports: each host whose installed
/// `builds` differs from this Mac's or that builds could not ask, and each
/// Mac whose sccache server launchd does not run (host_installed.rs), and
/// each Mac that lacks a setting that UI tests need or whose UI tests timed out
/// (ui_test_settings.rs). `assess` judges
/// only what this Mac's process reading finds, which runs no ssh, so `builds
/// health` and the dashboard each add these findings to the ones `assess`
/// returns.
pub fn host_findings(
    busy: &[crate::stuck::HostBusy],
    refusals: &[crate::host::Refusal],
    installed: &[crate::host_installed::HostInstalled],
) -> Vec<Finding> {
    crate::stuck::findings(busy)
        .into_iter()
        .map(|advice| Finding::new(FindingKind::Busy, advice))
        .chain(
            refusals
                .iter()
                // ui_test_settings::findings writes these with the host's
                // settings.
                .filter(|refusal| refusal.refused != crate::host::Refused::UiAutomation)
                .map(crate::host::refusal_finding)
                .chain(crate::host_installed::installed_findings(installed))
                .chain(crate::ui_test_settings::findings(refusals, installed))
                .map(|advice| Finding::new(FindingKind::Host, advice)),
        )
        .collect()
}

/// The finding when no dashboard runs (<dashboard> is its pid): the person
/// reads the worktrees and the lists there.
fn dashboard_finding(dashboard: Option<i32>) -> Option<Finding> {
    dashboard.is_none().then(|| {
        Finding::new(
            FindingKind::Dashboard,
            Advice::run(
                "dashboard: not running, so its page does not load; start it with".to_string(),
                "builds dashboard".to_string(),
            ),
        )
    })
}

/// `builds health [--json]`: exits 1 when something needs acting on.
pub fn cmd_health(tool: &Tool, args: &[String]) -> Result<()> {
    let (json, stop_junk) = match args {
        [] => (false, false),
        [flag] if flag == "--json" => (true, false),
        [flag] if flag == "--stop-junk" => (false, true),
        _ => crate::fail!("usage: builds health [--json | --stop-junk]"),
    };
    tool.places.require_volume()?;
    if stop_junk {
        // Judge the shells now, never from the daemon's last reading.
        let (lines, failed) = crate::shells::stop(&health(tool)?.shells, tool.places.uid);
        if lines.is_empty() {
            println!("no junk shells");
        }
        for line in lines {
            println!("{line}");
        }
        return if failed == 0 {
            Ok(())
        } else {
            Err(Fail::Exit(1))
        };
    }
    let mut health = match crate::daemon::client::health(&tool.places) {
        Some(health) => health,
        None => health(tool)?,
    };
    // builds health reads the busy processes itself: the daemon's health
    // holds none, and builds health prints every busy process, not only the
    // ones stuck.rs judges stuck.
    let ssh = crate::host::Ssh::from_env();
    let hosts = crate::host::all(&tool.places)?;
    let lock = tool.places.daemon_lock();
    let dashboard = crate::daemon::live::daemon_pid(&lock)
        .context(|| format!("reading the dashboard's pid from {}", lock.display()))?;
    // Each host's probe for its load, read with its busy processes: both
    // wait on an ssh round trip.
    let (busy, probes): (Vec<_>, Vec<_>) = parallel_map(&hosts, |host| {
        (
            crate::stuck::read(
                &ssh,
                host,
                host.target.is_none().then_some(dashboard).flatten(),
            ),
            crate::host::probe(&tool.places, &ssh, host),
        )
    })
    .into_iter()
    .unzip();
    health.findings.extend(dashboard_finding(dashboard));
    let ready = crate::host_load::ready_items(&crate::lists::store::load_all(&tool.places));
    let probed: Vec<_> = hosts.iter().cloned().zip(probes).collect();
    health.findings.extend(
        crate::host_load::findings(&probed, ready, tool.rules.floor_kib)
            .into_iter()
            .map(|advice| Finding::new(FindingKind::Load, advice)),
    );
    // Read now: a probe in another command, or this one, may have just
    // written or removed one.
    let refusals = crate::host::refusals(&tool.places)?;
    let installed = crate::host_installed::read_all_installed(&tool.places, &ssh, &hosts);
    health
        .findings
        .extend(host_findings(&busy, &refusals, &installed));
    if json {
        let mut value = serde_json::to_value(&health).expect("health serializes");
        value["busy"] = serde_json::json!(
            busy.iter()
                .map(|host| serde_json::json!({
                    "host": host.host,
                    "busy": host.read.as_ref().ok().map(|read| &read.busy),
                    "error": host.read.as_ref().err(),
                }))
                .collect::<Vec<_>>()
        );
        println!("{value}");
    } else {
        // The dashboard and the busy processes come before "needs action",
        // which is last.
        let mut out = lines(&health);
        let at = out
            .iter()
            .position(|line| line.starts_with("needs action"))
            .unwrap_or(out.len());
        let mut before = vec![match dashboard {
            Some(pid) => format!("dashboard: up (pid {pid})"),
            None => "dashboard: not running".to_string(),
        }];
        before.extend(crate::stuck::lines(&busy));
        out.splice(at..at, before);
        for line in out {
            println!("{line}");
        }
    }
    if health.findings.is_empty() {
        Ok(())
    } else {
        Err(Fail::Exit(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::side_git::Commit;

    /// Writers in one process never share a staged file: each write of the
    /// readings completes whatever the others do.
    #[test]
    fn writes_of_the_readings_in_one_process_all_complete() {
        let scratch = crate::testing::Scratch::new("readings-writers");
        let file = scratch.dir.join("readings.json");
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let file = file.clone();
                std::thread::spawn(move || {
                    for at in 0..100 {
                        write_readings(&file, &[Reading { executed: 1, at }]).unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
    }

    /// A claim with no change for hours and no build is stalled, unless its
    /// note says its agent holds the worktree or its design change waits on
    /// the person's review (the third claim).
    #[test]
    fn a_held_claim_is_not_idle() {
        let now = 1_800_000_000;
        let machine = Machine {
            at: now,
            load: [1.0; 3],
            cores: 8,
            builds: Vec::new(),
            remote_runs: Vec::new(),
            app_builds: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                listening: true,
                executed: Some(1),
                compared: None,
                waiting: Vec::new(),
            },
            shells: Shells::default(),
            locks: Vec::new(),
            queues: Vec::new(),
        };
        let place = |n: usize| Place {
            label: format!("demo/side-{n}"),
            repo: "demo".to_string(),
            top: PathBuf::from(format!("/demo-worktrees/side-{n}")),
            root: PathBuf::from(format!("/volume/demo/side-{n}")),
        };
        let claimed = |n: usize, state: Option<&str>| Claimed {
            place: n - 1,
            claim: Claim {
                who: format!("agent {n}"),
                since: now - 70 * 24 * 3600,
                note: Some("kept for later".to_string()),
                note_state: state.map(str::to_string),
            },
            branch: Some("work".to_string()),
            git: SideGit {
                head: None,
                ahead: Vec::new(),
                uncommitted: Some(3),
                last_write: Some(now - 3 * 3600),
                error: None,
            },
            awaiting_review: n == 3,
        };
        let health = assess(
            now,
            DiskSpace {
                free_kib: 100 << 20,
                floor_kib: 30 << 20,
                hard_floor_kib: 10 << 20,
            },
            machine,
            &[place(1), place(2), place(3)],
            vec![claimed(1, None), claimed(2, Some("held")), claimed(3, None)],
            LeftBehind::default(),
            Vec::new(),
        );
        assert_eq!(health.findings.len(), 1, "{:?}", health.findings);
        assert!(
            health.findings[0]
                .advice
                .text
                .starts_with("worktree: demo/side-1 "),
            "{:?}",
            health.findings
        );
        assert_eq!(
            health
                .worktrees
                .iter()
                .map(|w| w.stalled)
                .collect::<Vec<_>>(),
            [true, false, false],
            "the dashboard shows the finding's worktree, and only it, as stalled"
        );
    }

    /// The findings for one claimed worktree whose files last changed
    /// <changed_ago> seconds before now and whose claim began <claimed_ago>
    /// seconds before now.
    fn findings_for(claimed_ago: i64, changed_ago: i64) -> Vec<Finding> {
        let now = 1_800_000_000;
        let machine = Machine {
            at: now,
            load: [1.0; 3],
            cores: 8,
            builds: Vec::new(),
            remote_runs: Vec::new(),
            app_builds: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                listening: true,
                executed: Some(1),
                compared: None,
                waiting: Vec::new(),
            },
            shells: Shells::default(),
            locks: Vec::new(),
            queues: Vec::new(),
        };
        let place = Place {
            label: "demo/side-1".to_string(),
            repo: "demo".to_string(),
            top: PathBuf::from("/demo-worktrees/side-1"),
            root: PathBuf::from("/volume/demo/side-1"),
        };
        let claimed = Claimed {
            place: 0,
            claim: Claim {
                who: "agent 1".to_string(),
                since: now - claimed_ago,
                note: None,
                note_state: None,
            },
            branch: Some("work".to_string()),
            git: SideGit {
                head: None,
                ahead: Vec::new(),
                uncommitted: Some(3),
                last_write: Some(now - changed_ago),
                error: None,
            },
            awaiting_review: false,
        };
        assess(
            now,
            DiskSpace {
                free_kib: 100 << 20,
                floor_kib: 30 << 20,
                hard_floor_kib: 10 << 20,
            },
            machine,
            &[place],
            vec![claimed],
            LeftBehind::default(),
            Vec::new(),
        )
        .findings
    }

    /// Old files do not make a claim of the last few minutes quiet.
    #[test]
    fn a_worktree_claimed_less_than_an_hour_ago_is_not_quiet() {
        let findings = findings_for(10 * 60, 4 * 3600);
        assert!(findings.is_empty(), "{findings:?}");
    }

    /// When a claim is older than the quiet limit, health lists the worktree
    /// as quiet if its files have not changed for that long.
    #[test]
    fn a_worktree_claimed_long_ago_with_no_change_for_an_hour_is_quiet() {
        let findings = findings_for(5 * 3600, 2 * 3600);
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    /// A claimed worktree with no change for hours whose `builds cargo` runs
    /// cargo on another host is building there: it is not idle, and its line
    /// names the host.
    #[test]
    fn a_quiet_worktree_whose_cargo_runs_on_another_host_is_not_idle() {
        let now = 1_800_000_000;
        let machine = Machine {
            at: now,
            load: [1.0; 3],
            cores: 8,
            builds: Vec::new(),
            remote_runs: vec![RemoteRun {
                pid: 4242,
                place: 0,
                host: "m1".to_string(),
            }],
            app_builds: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                listening: true,
                executed: Some(1),
                compared: None,
                waiting: Vec::new(),
            },
            shells: Shells::default(),
            locks: Vec::new(),
            queues: Vec::new(),
        };
        let place = Place {
            label: "demo/side-1".to_string(),
            repo: "demo".to_string(),
            top: PathBuf::from("/demo-worktrees/side-1"),
            root: PathBuf::from("/volume/demo/side-1"),
        };
        let claimed = Claimed {
            place: 0,
            claim: Claim {
                who: "agent 1".to_string(),
                since: now - 5 * 3600,
                note: None,
                note_state: None,
            },
            branch: Some("work".to_string()),
            git: SideGit {
                head: None,
                ahead: Vec::new(),
                uncommitted: Some(3),
                last_write: Some(now - 2 * 3600),
                error: None,
            },
            awaiting_review: false,
        };
        let health = assess(
            now,
            DiskSpace {
                free_kib: 100 << 20,
                floor_kib: 30 << 20,
                hard_floor_kib: 10 << 20,
            },
            machine,
            &[place],
            vec![claimed],
            LeftBehind::default(),
            Vec::new(),
        );
        assert!(health.findings.is_empty(), "{:?}", health.findings);
        let shown = lines(&health);
        assert!(
            shown
                .iter()
                .any(|line| line.ends_with("; running cargo on host m1 (builds cargo, pid 4242)")),
            "{shown:?}"
        );
    }

    /// A claimed worktree with no change for hours is building while a
    /// `builds app-build` of its branch runs, wherever it was run from and
    /// wherever its lanes build; an app build of another branch run from
    /// elsewhere leaves it idle.
    #[test]
    fn a_quiet_worktree_whose_branch_an_app_build_builds_is_not_idle() {
        let now = 1_800_000_000;
        let run = |reference: &str| AppBuildRun {
            pid: 5151,
            repo: "demo".to_string(),
            reference: reference.to_string(),
            started_from: Some("demo/main".to_string()),
        };
        let judge = |app_builds: Vec<AppBuildRun>| {
            let machine = Machine {
                at: now,
                load: [1.0; 3],
                cores: 8,
                builds: Vec::new(),
                remote_runs: Vec::new(),
                app_builds,
                unread: Vec::new(),
                commits: Default::default(),
                sccache: SccacheHealth {
                    listening: true,
                    executed: Some(1),
                    compared: None,
                    waiting: Vec::new(),
                },
                shells: Shells::default(),
                locks: Vec::new(),
                queues: Vec::new(),
            };
            let place = Place {
                label: "demo/side-1".to_string(),
                repo: "demo".to_string(),
                top: PathBuf::from("/demo-worktrees/side-1"),
                root: PathBuf::from("/volume/demo/side-1"),
            };
            let claimed = Claimed {
                place: 0,
                claim: Claim {
                    who: "agent 1".to_string(),
                    since: now - 5 * 3600,
                    note: None,
                    note_state: None,
                },
                branch: Some("work".to_string()),
                git: SideGit {
                    head: None,
                    ahead: Vec::new(),
                    uncommitted: Some(0),
                    last_write: Some(now - 2 * 3600),
                    error: None,
                },
                awaiting_review: false,
            };
            assess(
                now,
                DiskSpace {
                    free_kib: 100 << 20,
                    floor_kib: 30 << 20,
                    hard_floor_kib: 10 << 20,
                },
                machine,
                &[place],
                vec![claimed],
                LeftBehind::default(),
                Vec::new(),
            )
        };
        let building = judge(vec![run("work")]);
        assert!(building.findings.is_empty(), "{:?}", building.findings);
        let shown = lines(&building);
        assert!(
            shown
                .iter()
                .any(|line| line.ends_with("; builds app-build of work (pid 5151)")),
            "{shown:?}"
        );
        assert_eq!(judge(vec![run("other")]).findings.len(), 1);
    }

    /// A process whose arguments builds could not read is a finding that
    /// says what health cannot tell without them.
    #[test]
    fn a_process_builds_could_not_read_is_a_finding() {
        let now = 1_800_000_000;
        let line = "builds cannot read the arguments of process 77, which still runs, so health cannot tell whether it is a `builds app-build` request";
        let machine = Machine {
            at: now,
            load: [1.0; 3],
            cores: 8,
            builds: Vec::new(),
            remote_runs: Vec::new(),
            app_builds: Vec::new(),
            unread: vec![line.to_string()],
            commits: Default::default(),
            sccache: SccacheHealth {
                listening: true,
                executed: Some(1),
                compared: None,
                waiting: Vec::new(),
            },
            shells: Shells::default(),
            locks: Vec::new(),
            queues: Vec::new(),
        };
        let health = assess(
            now,
            DiskSpace {
                free_kib: 100 << 20,
                floor_kib: 30 << 20,
                hard_floor_kib: 10 << 20,
            },
            machine,
            &[],
            Vec::new(),
            LeftBehind::default(),
            Vec::new(),
        );
        assert_eq!(
            health.findings,
            [Finding::new(
                FindingKind::Processes,
                Advice::said(format!("processes: {line}"))
            )]
        );
    }

    /// An unclaimed worktree holding commits missing from origin/main is
    /// work left behind; one holding none is not a finding.
    #[test]
    fn an_unclaimed_worktree_with_unlanded_commits_needs_action() {
        let now = 1_800_000_000;
        let machine = Machine {
            at: now,
            load: [1.0; 3],
            cores: 8,
            builds: Vec::new(),
            remote_runs: Vec::new(),
            app_builds: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                listening: true,
                executed: Some(1),
                compared: None,
                waiting: Vec::new(),
            },
            shells: Shells::default(),
            locks: Vec::new(),
            queues: Vec::new(),
        };
        let place = |n: usize| Place {
            label: format!("demo/side-{n}"),
            repo: "demo".to_string(),
            top: PathBuf::from(format!("/demo-worktrees/side-{n}")),
            root: PathBuf::from(format!("/volume/demo/side-{n}")),
        };
        let commit = Commit {
            sha: "abc".into(),
            full: "abc".into(),
            time: now,
            authored: now,
            subject: "Work".into(),
        };
        let git = |ahead: Vec<Commit>| SideGit {
            head: None,
            ahead,
            uncommitted: Some(0),
            last_write: None,
            error: None,
        };
        let health = assess(
            now,
            DiskSpace {
                free_kib: 100 << 20,
                floor_kib: 30 << 20,
                hard_floor_kib: 10 << 20,
            },
            machine,
            &[place(1), place(2)],
            Vec::new(),
            LeftBehind {
                worktrees: vec![
                    Unclaimed {
                        place: 0,
                        git: git(vec![commit]),
                        parked_unknown: Some("project demo's lists: broken".to_string()),
                    },
                    Unclaimed {
                        place: 1,
                        git: git(Vec::new()),
                        parked_unknown: None,
                    },
                ],
                branches: Vec::new(),
            },
            Vec::new(),
        );
        assert_eq!(health.findings.len(), 1, "{:?}", health.findings);
        assert!(
            health.findings[0]
                .advice
                .text
                .starts_with("worktree: demo/side-1 has no claim and 1 commit waiting to land"),
            "{:?}",
            health.findings
        );
        assert!(
            health.findings[0].advice.text.ends_with(
                "builds cannot tell whether an item is parked there, since project demo's lists: broken"
            ),
            "{:?}",
            health.findings
        );
    }

    /// A host finding keeps its command apart from its text, for the
    /// dashboard to show; a host whose processes builds could not read over
    /// ssh adds no finding.
    #[test]
    fn a_host_that_refuses_jobs_is_a_finding_that_names_its_command() {
        let refusal = crate::host::Refusal {
            host: "m1".to_string(),
            refused: crate::host::Refused::Jobs,
            at: 1_800_000_000,
            said: "git failed".to_string(),
        };
        let busy = vec![crate::stuck::HostBusy {
            host: "m1".to_string(),
            read: Err("ssh failed".to_string()),
        }];
        let found = host_findings(&busy, &[refusal], &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, FindingKind::Host);
        assert_eq!(found[0].advice.command.as_deref(), Some("builds host list"));
        assert!(
            found[0]
                .advice
                .line()
                .ends_with("to probe every host now, run `builds host list`"),
            "{}",
            found[0].advice.line()
        );
    }

    /// A host whose Xcode cannot start is a finding that names the fix
    /// xcodebuild named and the command that lets mac app builds use the host
    /// again.
    #[test]
    fn a_host_whose_xcode_cannot_start_names_the_fix_and_how_to_use_it_again() {
        let refusal = crate::host::Refusal {
            host: "m1".to_string(),
            refused: crate::host::Refused::Xcode,
            at: 1_800_000_000,
            said: "A required plugin failed to load. Please ensure system content is up-to-date, try running 'xcodebuild -runFirstLaunch'.".to_string(),
        };
        let found = host_findings(&[], &[refusal], &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, FindingKind::Host);
        assert_eq!(
            found[0].advice.command.as_deref(),
            Some("builds host clear-xcode m1")
        );
        let line = found[0].advice.line();
        assert!(
            line.starts_with("host m1's Xcode cannot start")
                && line.contains("Run sudo xcodebuild -runFirstLaunch on m1"),
            "{line}"
        );
    }

    /// A host whose UI tests timed out gets one line, the one
    /// ui_test_settings writes, not a second one from its record.
    #[test]
    fn a_host_whose_ui_tests_timed_out_is_one_finding() {
        let refusal = crate::host::Refusal {
            host: "m1".to_string(),
            refused: crate::host::Refused::UiAutomation,
            at: 1_800_000_000,
            said: "Timed out while enabling automation mode.".to_string(),
        };
        assert_eq!(
            crate::host::refusal_finding(&refusal).command.as_deref(),
            Some("builds host setup m1")
        );
        let found = host_findings(&[], &[refusal], &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].advice.command.as_deref(),
            Some("builds host setup m1")
        );
    }

    /// A dashboard that runs is no finding; none running is one that says
    /// how to start it.
    #[test]
    fn health_asks_for_a_dashboard_only_when_none_runs() {
        assert_eq!(dashboard_finding(Some(70525)), None);
        let found = dashboard_finding(None).expect("a finding");
        assert_eq!(found.kind, FindingKind::Dashboard);
        assert_eq!(found.advice.command.as_deref(), Some("builds dashboard"));
    }
}
