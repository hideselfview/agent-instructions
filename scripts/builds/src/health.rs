//! `builds health`: what on this machine is stuck or needs acting on, for a
//! coordinator that checks every few minutes. It reads; the one thing it
//! keeps is its own record of sccache's finished compiles, to tell next time
//! whether the server finished any since. Before it reads, it fixes what a
//! command fixes (`health_fix`): it releases idle claims, sends the judge
//! again the findings that waited for a settle and reached it in no message,
//! and forgets a judge whose session ended.
//!
//! It exits 1 when something needs acting on: clients waiting on an sccache
//! server that finished nothing since the last readings, agent shells that are
//! junk (shells.rs), a claimed worktree with no change and no build for an hour
//! (its own, a `builds cargo` run of its own on another host, a `builds
//! app-build` of its branch, or one it holds the app lock for), the volume
//! under its
//! free-space floor, a lists store that fails its check, a running item
//! whose worktree is gone or has no claim (lists/running.rs), a kept item
//! whose worktree is gone or whose waits are all met (lists/kept_work.rs), a held worktree
//! whose item waits on nothing open, commits that change design files no
//! recorded design change covers (lists/held.rs), a repo with a design folder
//! and no design globs (lists/project.rs), or a build host that refuses
//! jobs, whose Xcode cannot start, whose installed `builds` differs from this
//! Mac's or whose sccache server launchd does not run (host_installed.rs), or
//! that lacks a setting that UI tests need or whose UI tests timed out turning on
//! automation mode (ui_test_settings.rs), a host whose cores sit idle while
//! items are ready or whose load is well over its cores, or a ready item of project builds
//! that no agent runs (host_load.rs), a
//! project's main checkout whose main lacks commits that origin/main has
//! (main_checkout.rs), work that has run long on this Mac or a host, such as
//! an install waiting at its gates or a cargo job on a host (long_waits.rs),
//! or no `builds prune` that finished in twice the interval the dashboard
//! runs it at (prune_schedule.rs).

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
use crate::side_git::SideRead;
use crate::text::{human, local_minute, minutes_since};
use crate::tool::Tool;

/// An sccache client waiting this long is worth a look.
const CLIENT_MINUTES: i64 = 10;
/// A claimed worktree with no change and no build this long has stalled.
const QUIET_MINUTES: i64 = 60;

/// What git said of a side worktree, or what it said when the read failed.
pub type GitRead = std::result::Result<SideRead, String>;

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
    /// A build host that refuses work, lacks something builds sets up
    /// there, or shows a system alert in front of every app.
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
    /// A host near its limit of processes per user (proc_limit.rs).
    ProcessLimit,
    /// No `builds prune` finished lately (prune_schedule.rs).
    Prune,
    /// Work that has run long: an install waiting at its gates, a hook run,
    /// a landing, a cargo run or a job on a host (long_waits.rs).
    Waits,
    /// A process whose caller is gone (orphans.rs).
    Orphan,
    /// The build volume keeps a history of its file events (fsevents_log.rs).
    EventLog,
    /// A repo's cached shots runs that branches waiting to land need take
    /// more than the cache's cap (shots_cache.rs).
    ShotsCache,
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
    /// Does the coordinator's health-check schedule still have work to
    /// watch: an agent runs (a claimed worktree that is not held), or commits
    /// wait to land in a worktree or branch that is not held? False when
    /// every worktree with waiting commits is held and no agent runs.
    pub schedule_needed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SccacheHealth {
    pub server: SccacheServer,
    /// Clients waiting CLIENT_MINUTES or more.
    pub waiting: Vec<Client>,
}

/// What health read of the sccache server on this Mac.
#[derive(Clone, Serialize, Deserialize)]
pub enum SccacheServer {
    /// No process listens on sccache's port.
    Silent,
    /// A server listens, and reading its stats failed with this error.
    Unread(String),
    /// The server's count of executed compiles now ("Compile requests
    /// executed"), and the reading compared against: the newest one at least
    /// `sccache_readings::READING_MINUTES` old.
    Read {
        executed: u64,
        compared: Option<crate::sccache_readings::Reading>,
    },
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
    pub git: GitRead,
    /// Minutes since the newer of HEAD's commit and the newest uncommitted
    /// write.
    pub quiet_minutes: Option<i64>,
    /// Minutes since the newest of HEAD's commit, the newest uncommitted
    /// write and the start of the claim: the time `stalled` and the release
    /// of an idle claim (health_fix.rs) judge.
    pub idle_minutes: i64,
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
    /// The `builds land` processes that land in it, running or waiting for
    /// another landing.
    pub landings: Vec<LandingRun>,
    /// The holder of an app lock building for this worktree in the app
    /// checkout (`Holder::builds_for`).
    pub app_build: Option<i32>,
    /// Its app builds that wait in a lane's queue (`Queued::builds_for`).
    pub in_line: Vec<InLine>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LockHealth {
    pub repo: String,
    /// The app lane the lock is for.
    #[serde(with = "crate::lane::as_word")]
    pub lane: Lane,
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
    #[serde(with = "crate::lane::as_word")]
    pub lane: Lane,
    pub queue: Vec<Queued>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Queued {
    pub pid: i32,
    pub minutes: i64,
    pub command: String,
    /// The checkout its working directory is in.
    pub started_from: Option<String>,
    /// The commit it builds, when it is a lane's process of a `builds
    /// app-build` request (`app_build::lane_requested`).
    pub commit: Option<String>,
}

/// An app build for a worktree that waits in a lane's queue: the queue's
/// repo and lane, the waiting process, and its place in line.
#[derive(Clone, Serialize, Deserialize)]
pub struct InLine {
    pub repo: String,
    #[serde(with = "crate::lane::as_word")]
    pub lane: Lane,
    pub pid: i32,
    /// 1 for the next to take a lock.
    pub place: usize,
    /// How many wait in the queue.
    pub of: usize,
}

impl Queued {
    /// Does it build for the worktree <label> at <git>? When it was run from
    /// there, or it builds a commit of that worktree's own (`SideRead::owns`).
    fn builds_for(&self, label: &str, git: &GitRead) -> bool {
        self.started_from.as_deref() == Some(label)
            || self
                .commit
                .as_deref()
                .is_some_and(|commit| git.as_ref().is_ok_and(|git| git.owns(commit)))
    }
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

/// A `builds land` process that lands in a worktree: running, or waiting for
/// the turn of another landing (land_turns.rs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LandingRun {
    pub pid: i32,
    /// Index into the places it was placed among.
    #[serde(skip)]
    pub place: usize,
    pub waits: LandingWait,
}

/// Whether a landing waits for another, from the turn files (land_turns.rs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LandingWait {
    /// No other landing holds a turn it takes.
    Nothing,
    /// The landing that holds the turn it takes.
    For(String),
    /// Builds could not read the turn files, and `Machine::unread` says why.
    Unread,
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
    fn builds_for(&self, label: &str, git: &GitRead) -> bool {
        self.started_from.as_deref() == Some(label)
            || self
                .app_head
                .as_deref()
                .is_some_and(|head| git.as_ref().is_ok_and(|git| git.owns(head)))
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
    /// The `builds land` processes that run, including those that wait for
    /// another landing.
    pub landings: Vec<LandingRun>,
    /// What builds could not read of the processes it needed, one line each.
    pub unread: Vec<String>,
    /// The `git commit`s that run, which keep their worktree from being free.
    pub commits: crate::procs::GitCommits,
    pub sccache: SccacheHealth,
    pub shells: Shells,
    pub locks: Vec<LockHealth>,
    pub queues: Vec<LaneQueue>,
}

/// What the sccache server on this Mac says now: its count of executed
/// compiles and the newest reading of that server at least
/// `sccache_readings::READING_MINUTES` old; this reading joins the record
/// when the newest of this Mac in it is a minute old or more
/// (sccache_readings.rs).
pub fn sccache_reading(home: &Path, now: i64) -> Result<SccacheServer> {
    use crate::sccache_readings::{Reading, compared, read, record, write};
    let Some(server) = crate::sccache::listener()? else {
        return Ok(SccacheServer::Silent);
    };
    let stats = match crate::sccache::running_stats() {
        Ok(stats) => stats,
        Err(error) => return Ok(SccacheServer::Unread(error.text())),
    };
    let server = server as i32;
    let mut readings = read(home)?;
    let before = compared(&readings, crate::host::LOCAL, server, now).cloned();
    if record(
        &mut readings,
        Reading::of(crate::host::LOCAL, server, now, &stats),
    ) {
        write(home, &readings)?;
    }
    Ok(SccacheServer::Read {
        executed: stats.executed,
        compared: before,
    })
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
    machine: &crate::places::Places,
    now: i64,
    sccache: SccacheServer,
) -> Result<Machine> {
    let processes = Processes::read()?;
    let builds = build_processes(&processes, places, &BUILD_NAMES);
    let (remote_runs, mut unread) = crate::cargo_place::remote_runs(&processes, places, hosts);
    let mut app_builds = Vec::new();
    let mut landings = Vec::new();
    for process in processes.list.iter().filter(|p| p.name == "builds") {
        let argv = match live_args(process.pid) {
            Ok(Some(argv)) => argv,
            Ok(None) => continue,
            Err(error) => {
                unread.push(format!(
                    "{error}, so health cannot tell whether it is a `builds app-build` request or a landing"
                ));
                continue;
            }
        };
        if let Some((repo, reference)) = crate::app_build::requested(&argv) {
            app_builds.push(AppBuildRun {
                pid: process.pid,
                repo: repo.to_string(),
                reference: reference.to_string(),
                started_from: started_from(places, process.pid),
            });
        } else if let Some(worktree) = crate::land_turns::landing_worktree(process.pid, &argv) {
            let worktree = match worktree {
                Ok(worktree) => worktree,
                Err(text) => {
                    unread.push(text);
                    continue;
                }
            };
            if let Some(place) = crate::procs::place_of(places, &worktree) {
                let waits = match crate::land_turns::waits_for(&worktree, process.pid, &argv) {
                    Ok(None) => LandingWait::Nothing,
                    Ok(Some(whose)) => LandingWait::For(whose.to_string()),
                    Err(error) => {
                        unread.push(format!(
                            "{}, so health cannot tell whether the landing of process {} waits for another",
                            error.text(),
                            process.pid
                        ));
                        LandingWait::Unread
                    }
                };
                landings.push(LandingRun {
                    pid: process.pid,
                    place,
                    waits,
                });
            }
        }
    }
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
                    let words = crate::ack::agent::without_flag(&argv);
                    words
                        .get(1)
                        .is_some_and(|a| matches!(*a, "lock" | "build-app" | "app-build"))
                        && words.get(2).is_some_and(|r| *r == repo.as_str())
                };
                args(process.pid).is_some_and(locking) && opened(process.pid)
            });
        // A build whose `builds lock` was killed outright still holds the
        // lock: its processes have the file open. Found only when the lock is
        // taken, since reading every process's open files costs.
        let driver_running = holding.is_some();
        if holding.is_none() && taken(path) {
            holding = topmost(&processes, machine.uid, opened);
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
            lane: *lane,
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
        if queues.iter().any(|q| q.repo == *repo && q.lane == *lane) {
            continue;
        }
        // `Queue::waiting` is the tickets whose builds hold no lock.
        let in_line = match path.parent().and_then(|locks_dir| locks_dir.parent()) {
            Some(repo_dir) => match crate::queue::Queue::in_repo_dir(repo_dir, *lane).waiting() {
                Ok(in_line) => in_line,
                Err(error) => {
                    unread.push(format!(
                        "{}, so health cannot tell who waits in {repo}'s {} lane",
                        error.text(),
                        lane.word()
                    ));
                    Vec::new()
                }
            },
            None => {
                unread.push(format!(
                    "{} has no repo folder, so health cannot tell who waits in {repo}'s {} lane",
                    path.display(),
                    lane.word()
                ));
                Vec::new()
            }
        };
        let mut queue = Vec::new();
        for process in in_line.into_iter().filter_map(|pid| {
            processes
                .list
                .iter()
                .find(|p| p.pid == pid && p.name == "builds")
        }) {
            let commit = match live_args(process.pid) {
                Ok(argv) => argv
                    .and_then(|argv| crate::app_build::lane_requested(&argv).map(str::to_string)),
                Err(error) => {
                    unread.push(format!(
                        "{error}, so health cannot tell which commit it waits to build in {repo}'s {} lane",
                        lane.word()
                    ));
                    None
                }
            };
            queue.push(Queued {
                pid: process.pid,
                minutes: minutes_since(now, process.started),
                command: command_line(process.pid),
                started_from: started_from(places, process.pid),
                commit,
            });
        }
        queues.push(LaneQueue {
            repo: repo.clone(),
            lane: *lane,
            queue,
        });
    }
    let queued: std::collections::HashSet<i32> = queues
        .iter()
        .flat_map(|q| q.queue.iter().map(|w| w.pid))
        .collect();
    let sessions = match crate::claude_sessions::pids(&machine.claude_sessions()) {
        Ok(sessions) => sessions,
        Err(error) => crate::fail!(
            "reading the running Claude Code sessions failed, so health cannot tell which shells a session waits on: {error}"
        ),
    };
    let shells = crate::shells::judge(
        &crate::shells::gather(&processes, machine.uid),
        now,
        &machine.home,
        places,
        &queued,
        &sessions,
    );
    Ok(Machine {
        at: now,
        load: load_average(),
        cores: cores(),
        builds,
        remote_runs,
        app_builds,
        landings,
        unread,
        commits: crate::procs::git_commits(&processes),
        sccache: SccacheHealth {
            server: sccache,
            waiting,
        },
        shells,
        locks: held,
        queues,
    })
}

/// The label of the checkout the working directory of process <pid> is in.
fn started_from(places: &[Place], pid: i32) -> Option<String> {
    place_of_process(places, pid).map(|at| places[at].label.clone())
}

/// Is the lock at <path> taken? Asked by taking it without waiting and
/// letting it go at once.
fn taken(path: &Path) -> bool {
    match crate::queue::open_with(path, libc::O_NONBLOCK | libc::O_CLOEXEC) {
        Ok(fd) => {
            unsafe { libc::close(fd) };
            false
        }
        Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
    }
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
    pub git: GitRead,
    /// Do its commits wait on the person's review of a design change: one
    /// that waits on their approval covers them, and the rules review takes
    /// them (decision #362)?
    pub awaiting_review: bool,
    /// What blocks its branch from landing, from the review records; None
    /// when health did not read them (nothing waits to land there).
    pub landing: Option<Advice>,
}

/// A side worktree with no claim that holds commits missing from origin/main:
/// work an agent left behind. Never the app checkout, which no one claims:
/// what it has checked out is the branch an app build is building, and never
/// a worktree an open item is kept in, whose work waits there for the item
/// (lists/kept_work.rs).
pub struct Unclaimed {
    pub place: usize,
    pub git: GitRead,
    /// Why builds cannot tell whether an item is kept there: a store that
    /// does not load. None when builds read every store.
    pub kept_unknown: Option<String>,
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
        match &s.server {
            SccacheServer::Silent => findings.push(Finding::new(
                FindingKind::Sccache,
                Advice::said(format!(
                    "sccache: no server is listening and {} clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m)",
                    s.waiting.len()
                )),
            )),
            SccacheServer::Unread(error) => findings.push(Finding::new(
                FindingKind::Sccache,
                Advice::said(format!(
                    "sccache: {} clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m), and health cannot tell whether the server finishes compiles, because reading its stats failed: {error}",
                    s.waiting.len()
                )),
            )),
            SccacheServer::Read {
                executed,
                compared: Some(before),
            } if *executed == before.executed => findings.push(Finding::new(
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
    let mut schedule_needed = !left.branches.is_empty();
    for Unclaimed {
        place,
        git,
        kept_unknown,
    } in left.worktrees
    {
        // A worktree git could not read, or one whose kept items builds
        // could not read, may hold commits health did not see.
        schedule_needed |= git.is_err() || kept_unknown.is_some();
        let ahead = git.as_ref().map_or(0, |git| git.ahead.len());
        if ahead > 0 {
            schedule_needed = true;
            let unknown = match kept_unknown {
                Some(fault) => {
                    format!("; builds cannot tell whether an item is kept there, since {fault}")
                }
                None => String::new(),
            };
            findings.push(Finding::new(
                FindingKind::Unclaimed,
                Advice::said(format!(
                    "worktree: {} has no claim and {} waiting to land, so it is not retired{unknown}",
                    places[place].label,
                    crate::text::counted(ahead, "commit", "commits")
                )),
            ));
        }
    }
    for branch in &left.branches {
        findings.push(Finding::new(FindingKind::Branch, loose_advice(branch)));
    }
    let mut worktrees = Vec::new();
    for Claimed {
        place,
        claim,
        branch,
        git,
        awaiting_review,
        landing,
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
        let landings: Vec<LandingRun> = machine
            .landings
            .iter()
            .filter(|run| run.place == place)
            .cloned()
            .collect();
        let activity = git.as_ref().ok().and_then(SideRead::activity);
        let quiet_minutes = activity.map(|when| minutes_since(now, when));
        // A worktree claimed a few minutes ago is not idle, even when its
        // files have not changed for hours.
        let idle_minutes = minutes_since(
            now,
            activity.map_or(claim.since, |when| when.max(claim.since)),
        );
        let app_build = machine
            .locks
            .iter()
            .filter_map(|lock| lock.holder.as_ref())
            .find(|h| h.builds_for(&label, &git))
            .map(|h| h.pid);
        let in_line: Vec<InLine> = machine
            .queues
            .iter()
            .flat_map(|queue| {
                queue
                    .queue
                    .iter()
                    .enumerate()
                    .filter(|(_, waiting)| waiting.builds_for(&label, &git))
                    .map(|(at, waiting)| InLine {
                        repo: queue.repo.clone(),
                        lane: queue.lane,
                        pid: waiting.pid,
                        place: at + 1,
                        of: queue.queue.len(),
                    })
            })
            .collect();
        // A held claim is quiet on purpose: its note says why. A worktree
        // whose design change waits on the person's review is waiting on them.
        let held = claim.is_held();
        schedule_needed |= !held || git.is_err();
        let stalled = !held
            && !awaiting_review
            && building.is_empty()
            && remote_runs.is_empty()
            && app_builds.is_empty()
            && landings.is_empty()
            && app_build.is_none()
            && in_line.is_empty()
            && idle_minutes >= QUIET_MINUTES;
        if stalled {
            let quiet = format!(
                "worktree: {label} ({}) has had no change for {idle_minutes}m and no build runs there",
                claim.who
            );
            findings.push(Finding::new(
                FindingKind::Quiet,
                match landing {
                    Some(Advice { text, command }) => Advice {
                        text: format!("{quiet}; {text}"),
                        command,
                    },
                    None => Advice::said(quiet),
                },
            ));
        }
        worktrees.push(WorktreeHealth {
            checkout: label,
            path: places[place].top.display().to_string(),
            claim,
            branch,
            git,
            quiet_minutes,
            idle_minutes,
            stalled,
            building,
            remote_runs,
            app_builds,
            landings,
            app_build,
            in_line,
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
        schedule_needed,
    }
}

/// What health says of each repo whose cached shots runs that branches
/// waiting to land name take more than the cap, or of the failure to read
/// them.
fn shots_cache_findings(tool: &Tool) -> Vec<Finding> {
    let advice = match crate::shots_cache::over_cap_in_use(tool) {
        Ok(over) => over
            .into_iter()
            .map(|over| {
                Advice::said(format!(
                    "shots cache: the runs that {} still need take {} of {}'s folder in shots, over its {} cap; prune keeps them until those branches land",
                    over.branches.join(", "),
                    human(over.kib),
                    over.repo,
                    human(crate::shots_cache::CAP_KIB)
                ))
            })
            .collect(),
        Err(error) => vec![Advice::said(format!(
            "shots cache: builds cannot tell which runs branches need: {}",
            error.text()
        ))],
    };
    advice
        .into_iter()
        .map(|advice| Finding::new(FindingKind::ShotsCache, advice))
        .collect()
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

/// What the lists say, one line each: each fault <integrity> holds, which
/// SQLite's integrity checks found in a store's database, the running-item
/// and held-work findings about the claimed worktrees (lists/running.rs,
/// lists/held.rs),
/// each kept item whose worktree is gone (lists/kept_work.rs), and each repo with a design folder and no design globs
/// (lists/project.rs). Also
/// marks the worktrees whose design change waits on the person's review,
/// which a change that waits on their approval does once the rules review
/// takes the worktree's commits, so <claimed> is judged with that known. Both read the worktrees' design
/// commits through <reads>.
pub fn lists_findings(
    places_in: &crate::places::Places,
    integrity: Vec<String>,
    loaded: &std::result::Result<Vec<crate::lists::store::Loaded>, String>,
    places: &[Place],
    claimed: &mut [Claimed],
    worktrees: &[crate::lists::held::Worktree],
    reads: WorktreeReads,
) -> Result<Vec<Advice>> {
    let WorktreeReads { designs, reviews } = reads;
    let approving = crate::lists::held::awaiting_approval(loaded, worktrees, designs);
    for one in claimed.iter_mut() {
        let landing = worktrees
            .iter()
            .find(|worktree| worktree.label == places[one.place].label)
            .and_then(|worktree| worktree.landing(places_in, loaded, reviews));
        one.awaiting_review = approving.contains(&places[one.place].label)
            && matches!(landing, Some(Ok(crate::review::idle::Landing::Free(_))));
        one.landing = landing.map(|landing| match landing {
            Ok(landing) => landing.advice(),
            Err(fault) => Advice::said(format!(
                "builds cannot tell what blocks its branch from landing: {fault}"
            )),
        });
    }
    let mut lists = crate::lists::running::findings(integrity, loaded)?;
    lists.extend(crate::lists::kept_work::findings(loaded)?);
    lists.extend(crate::lists::held::findings(loaded, worktrees, designs));
    lists.extend(crate::lists::rust_check::findings(places_in));
    lists.extend(crate::lists::project::missing_globs_findings(
        &places_in.home,
        loaded,
    ));
    Ok(lists)
}

/// What the reading of the worktrees' commits keeps between readings: their
/// design commits and the verdict on each one's landing.
#[derive(Clone, Copy)]
pub struct WorktreeReads<'a> {
    pub designs: &'a crate::lists::design_reads::DesignReads,
    pub reviews: &'a crate::review::verdicts::Reads,
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
        &tool.places,
        now,
        sccache,
    )?;
    let mut claimed = Vec::new();
    let mut unclaimed = Vec::new();
    let mut unreadable = Vec::new();
    let loaded = crate::lists::store::load_all(&tool.places);
    // While a store does not load, health names each unclaimed worktree
    // with commits and says it cannot tell whether an item is kept there.
    let kept = crate::lists::kept_work::kept_worktrees(&loaded);
    for (at, root) in live.iter().enumerate() {
        let checkout = root.live.as_ref().expect("live");
        if !checkout.is_side() {
            continue;
        }
        match Claim::read(&checkout.claim_file()) {
            Ok(Some(claim)) => claimed.push((at, checkout, claim)),
            Ok(None) if root.is_app_checkout(&tool.places)? => {}
            Ok(None)
                if kept
                    .as_ref()
                    .is_ok_and(|kept| kept.contains(&checkout.top.display().to_string())) => {}
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
            kept_unknown: kept.as_ref().err().cloned(),
        })
        .collect();
    let gits = parallel_map(&claimed, |(_, checkout, _)| {
        crate::side_git::read(&checkout.top)
    });
    let mut judged = Vec::new();
    let mut held_worktrees = Vec::new();
    for ((place, checkout, claim), git) in claimed.into_iter().zip(gits) {
        let branch = checkout.branch()?;
        held_worktrees.push(crate::lists::held::Worktree::read(
            &tool.places,
            places[place].label.clone(),
            checkout,
            &claim,
            branch.clone(),
            &git,
            &crate::lists::held::Readings {
                commits: &machine.commits,
                loaded: &loaded,
            },
        )?);
        judged.push(Claimed {
            place,
            claim,
            branch,
            git,
            awaiting_review: false,
            landing: None,
        });
    }
    let lists = lists_findings(
        &tool.places,
        crate::lists::store::integrity(&tool.places),
        &loaded,
        &places,
        &mut judged,
        &held_worktrees,
        WorktreeReads {
            designs: &crate::lists::design_reads::DesignReads::with_program(tool.program.clone()),
            reviews: &crate::review::verdicts::Reads::with_root(Ok(
                crate::review::installed_rules_root(&tool.places.home),
            )),
        },
    )?;
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
    // A claim or a repo's branches that builds could not read may hide a
    // running agent or commits waiting to land.
    health.schedule_needed |= !unreadable.is_empty();
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

/// <n> as an ordinal: 1st, 2nd, 3rd, 4th, 11th, 21st.
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// What builds for worktree <w>: its build processes here, its `builds
/// cargo` and `builds lake` runs on other hosts, the `builds app-build` requests for it, the
/// app lock's holder building for it, and its app builds waiting in a lane's
/// queue with their places in line.
fn builds_line(w: &WorktreeHealth) -> String {
    let mut parts = Vec::new();
    if !w.building.is_empty() {
        parts.push(format!("building: {}", process_summary(&w.building)));
    }
    for run in &w.remote_runs {
        parts.push(format!(
            "running {command} on host {} (builds {command}, pid {})",
            run.host,
            run.pid,
            command = run.command
        ));
    }
    for run in &w.app_builds {
        parts.push(format!(
            "builds app-build of {} (pid {})",
            run.reference, run.pid
        ));
    }
    for landing in &w.landings {
        parts.push(match &landing.waits {
            LandingWait::For(whose) => format!(
                "builds land waits for another landing (pid {}; {whose})",
                landing.pid
            ),
            LandingWait::Nothing => format!("builds land runs (pid {})", landing.pid),
            LandingWait::Unread => format!(
                "builds land runs (pid {}), and builds could not tell whether it waits",
                landing.pid
            ),
        });
    }
    if let Some(pid) = w.app_build {
        parts.push(format!("building in the app checkout (pid {pid})"));
    }
    for waiting in &w.in_line {
        parts.push(format!(
            "app build waiting in {}'s {} lane, {} of {} in line (pid {})",
            waiting.repo,
            waiting.lane.word(),
            ordinal(waiting.place),
            waiting.of,
            waiting.pid
        ));
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
    let mut line = match &s.server {
        SccacheServer::Silent => "sccache: no server listening".to_string(),
        SccacheServer::Unread(error) => {
            format!("sccache: reading the server's stats failed: {error}")
        }
        SccacheServer::Read { executed, compared } => match compared {
            Some(before) => format!(
                "sccache: {executed} compiles executed, {} since {} ({}m ago)",
                executed.saturating_sub(before.executed),
                local_minute(before.at),
                minutes_since(health.generated_at, before.at)
            ),
            None => format!(
                "sccache: {executed} compiles executed; no reading {}+ min old to compare with yet",
                crate::sccache_readings::READING_MINUTES
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
        let git = match &w.git {
            Ok(git) => format!(
                "{} uncommitted, {} waiting to land",
                git.uncommitted,
                git.ahead.len()
            ),
            Err(error) => format!("git: {error}"),
        };
        out.push(format!(
            "    {}  [{}]  {} since {}{}",
            w.checkout,
            w.branch.as_deref().unwrap_or("detached"),
            w.claim.who,
            local_minute(w.claim.since),
            match (w.claim.state(), w.claim.note_text()) {
                (Some(state), note) =>
                    format!(" ({}): {}", state.replace('-', " "), note.unwrap_or("")),
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
                lock.lane.word(),
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
                lock.repo,
                lock.lane.word(),
                lock.checkout
            )),
        }
    }
    for q in &health.queues {
        out.push(format!(
            "app queue {} ({}): {} waiting",
            q.repo,
            q.lane.word(),
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
    out.push(if health.schedule_needed {
        "health schedule: needed".to_string()
    } else {
        "health schedule: can stop".to_string()
    });
    out
}

/// What health read of the hosts as a whole for `host_findings`.
pub struct Scanned {
    /// Each host's scan (host_scan.rs), this Mac's among them.
    pub scans: Vec<(Host, std::result::Result<crate::host_scan::Scan, String>)>,
    /// What a running install waits for (install_gate.rs).
    pub install: std::result::Result<Option<crate::install_gate::Waiting>, String>,
    /// The processes whose caller is gone (orphans.rs) that health has not
    /// stopped.
    pub orphans: Vec<crate::orphans::Orphan>,
    /// Each host's sccache server (sccache_queue.rs), or why health could
    /// not judge them.
    pub caches: std::result::Result<Vec<crate::sccache_queue::Cache>, String>,
    /// Seconds since 1970.
    pub now: i64,
}

impl Scanned {
    /// Judges each host's <scans>, given this Mac's processes as <holders>
    /// read them before the scans, at <now>: what a running install waits
    /// for, the processes whose caller is gone, and each host's compile
    /// cache, whose reading joins the record under <places>' home.
    pub fn judge(
        places: &crate::places::Places,
        scans: Vec<(Host, std::result::Result<crate::host_scan::Scan, String>)>,
        holders: &crate::job_claim::Holders,
        now: i64,
    ) -> Scanned {
        Scanned {
            install: crate::install_gate::waiting(places).map_err(|fault| fault.text()),
            orphans: crate::orphans::find(&scans, holders),
            caches: crate::sccache_queue::judge(&places.home, &scans, now)
                .map_err(|fault| fault.text()),
            scans,
            now,
        }
    }
}

/// The findings that come from the hosts: from <scanned>, the work that has
/// run long on this Mac or a host (long_waits.rs), each process whose caller
/// is gone that health left running (orphans.rs), and each host with more
/// compile work than its sccache server runs (sccache_queue.rs); and each
/// process that looks stuck on
/// one (stuck.rs), each host that refuses jobs or whose Xcode cannot start,
/// what `host_installed::installed_findings` reports: each host whose installed
/// `builds` differs from this Mac's or that builds could not ask, and each
/// Mac whose sccache server launchd does not run (host_installed.rs), and
/// each Mac that lacks a setting that UI tests need or whose UI tests timed out
/// (ui_test_settings.rs), and each Mac whose front app is a system alert
/// (host_front_app.rs), and each Mac whose screen is locked
/// (host_screen_lock.rs). `assess` judges
/// only what this Mac's process reading finds, which runs no ssh, so `builds
/// health` and the dashboard each add these findings to the ones `assess`
/// returns.
pub fn host_findings(
    scanned: &Scanned,
    busy: &[crate::stuck::HostBusy],
    refusals: &[crate::host::Refusal],
    installed: &[crate::host_installed::HostInstalled],
) -> Vec<Finding> {
    let checkout = |pid: i32| {
        crate::procs::cwd(pid)
            .and_then(|dir| crate::stuck::checkout_label(&dir.display().to_string()))
    };
    let caches = match &scanned.caches {
        Ok(caches) => crate::sccache_queue::findings(caches),
        Err(error) => vec![Advice::said(format!(
            "sccache: health could not judge the hosts' compile caches: {error}"
        ))],
    };
    let landing_wait = |pid: i32| {
        let Some(argv) = crate::procs::live_args(pid)? else {
            return Ok(None);
        };
        match crate::land_turns::landing_worktree(pid, &argv) {
            Some(worktree) => {
                crate::land_turns::waits_for(&worktree?, pid, &argv).map_err(|e| e.text())
            }
            None => Ok(None),
        }
    };
    crate::long_waits::findings(
        &scanned.scans,
        &scanned.install,
        &checkout,
        &landing_wait,
        scanned.now,
    )
    .into_iter()
    .map(|advice| Finding::new(FindingKind::Waits, advice))
    .chain(
        scanned
            .orphans
            .iter()
            .map(|orphan| Finding::new(FindingKind::Orphan, orphan.finding())),
    )
    .chain(
        caches
            .into_iter()
            .map(|advice| Finding::new(FindingKind::Sccache, advice)),
    )
    .chain(
        crate::stuck::findings(busy)
            .into_iter()
            .map(|advice| Finding::new(FindingKind::Busy, advice)),
    )
    .chain(
        refusals
            .iter()
            // ui_test_settings::findings writes these with the host's
            // settings.
            .filter(|refusal| refusal.refused != crate::host::Refused::UiAutomation)
            .map(crate::host::refusal_finding)
            .chain(crate::host_installed::installed_findings(installed))
            .chain(crate::ui_test_settings::findings(refusals, installed))
            .chain(crate::host_front_app::findings(&scanned.scans))
            .chain(crate::host_screen_lock::findings(&scanned.scans))
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
    if !stop_junk {
        // Before health reads the claims, so the reading shows what is left.
        match crate::health_fix::release_idle_claims(tool) {
            Ok(lines) => lines.iter().for_each(crate::text::note),
            Err(error) => crate::text::note(format!(
                "could not check which claims nothing holds: {}",
                error.text()
            )),
        }
    }
    if !stop_junk {
        match crate::health_fix::tell_judges(tool) {
            Ok(lines) => lines.iter().for_each(crate::text::note),
            Err(error) => crate::text::note(format!(
                "could not check which findings wait for a judge: {}",
                error.text()
            )),
        }
    }
    if stop_junk {
        // Judge the shells now, never from the daemon's last reading.
        let (mut lines, mut failed) = crate::shells::stop(&health(tool)?.shells, tool.places.uid)?;
        if lines.is_empty() {
            println!("no junk shells");
        }
        let (zombie_lines, zombie_failed) = crate::proc_limit::stop(tool.places.uid)?;
        lines.extend(zombie_lines);
        failed += zombie_failed;
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
    // A stalled claim that holds no work is released, and its item stopped,
    // from this reading; health reads again so the lists show what is left.
    match crate::health_fix::release_stalled_claims(tool, &health) {
        Ok(lines) if lines.is_empty() => {}
        Ok(lines) => {
            lines.iter().for_each(crate::text::note);
            health = self::health(tool)?;
        }
        Err(error) => crate::text::note(format!(
            "could not check which stalled claims hold nothing: {}",
            error.text()
        )),
    }
    // builds health reads the busy processes itself: the daemon's health
    // holds none, and builds health prints every busy process, not only the
    // ones stuck.rs judges stuck.
    let ssh = crate::host::Ssh::from_env();
    let mut hosts = crate::host::all(&tool.places)?;
    let lock = tool.places.daemon_lock();
    let dashboard = crate::daemon::live::daemon_pid(&lock)
        .context(|| format!("reading the dashboard's pid from {}", lock.display()))?;
    // Each host's probe for its load, read with its scan and busy processes:
    // each waits on an ssh round trip.
    // Read before the scans, so a job that a process here started after
    // this reading counts as running (job_claim.rs).
    let holders = crate::job_claim::Holders::read()?;
    let mut scans = Vec::new();
    let mut busy = Vec::new();
    let mut probes = Vec::new();
    for (scan, host_busy, probe) in parallel_map(&hosts, |host| {
        let scan = crate::host_scan::read(&ssh, host, &tool.places.home);
        let host_busy = crate::stuck::read(
            &ssh,
            host,
            &scan,
            host.target.is_none().then_some(dashboard).flatten(),
        );
        (
            scan,
            host_busy,
            crate::host::probe(&tool.places, &ssh, host),
        )
    }) {
        scans.push(scan);
        busy.push(host_busy);
        probes.push(probe);
    }
    let fixed = crate::health_fix::fix_hosts(
        &mut hosts,
        &mut probes,
        tool.rules.floor_kib,
        &crate::health_fix::Fixes {
            prune: &|host| {
                if host.target.is_none() {
                    return crate::prune::cmd_prune(tool, &[]).map_err(|fault| fault.text());
                }
                crate::host_setup::ask_builds_there(&tool.places, &ssh, host, "prune", "pruning")
                    .map_err(|fault| fault.text())?
                    .map(|_| ())
            },
            probe: &|host| crate::host::probe(&tool.places, &ssh, host),
            measure: &|host| {
                let target = host.target.as_deref().expect("a host to measure is remote");
                let os = crate::host::probe_target(&ssh, target)?.platform.os;
                // Health runs with no person at a terminal, so a sudo step
                // that setup still lacks fails and says what to run.
                crate::host_setup::setup_host(&tool.places, &ssh, host, os, false, &tool.program)
                    .map(|set_up| set_up.speed)
                    .map_err(|fault| fault.text())
            },
            stop_idle: &|host, pressure| {
                crate::idle_daemons::stop_on(&tool.places, &ssh, host, pressure, tool.now)
            },
        },
    );
    fixed.iter().for_each(crate::text::note);
    for (host, probe) in hosts.iter().zip(&probes) {
        if probe.is_err() {
            continue;
        }
        let place = crate::host::place(&host.name);
        match crate::run_tmp::sweep(&tool.places, &ssh, host, &holders) {
            Ok(swept) => {
                for folder in swept.deleted {
                    crate::text::note(format!(
                        "deleted the temporary folder {folder} on {place}: the run that made it ended"
                    ));
                }
                for entry in swept.unexpected {
                    crate::text::note(format!(
                        "{place} has an entry in its run folders that no run of builds made: {entry}"
                    ));
                }
            }
            Err(fault) => crate::text::note(fault.text()),
        }
    }
    health.findings.extend(dashboard_finding(dashboard));
    let (prune_line, prune_late) = crate::prune_schedule::health(&tool.places, tool.now);
    health.findings.extend(
        prune_late
            .into_iter()
            .map(|advice| Finding::new(FindingKind::Prune, advice)),
    );
    health.findings.extend(shots_cache_findings(tool));
    health
        .findings
        .extend(crate::fsevents_log::finding(&tool.places));
    let loaded = crate::lists::store::load_all(&tool.places);
    let ready = crate::host_load::ready_items(&loaded);
    let probed: Vec<_> = hosts.iter().cloned().zip(probes).collect();
    let mut scanned = Scanned::judge(
        &tool.places,
        hosts.iter().cloned().zip(scans).collect(),
        &holders,
        tool.now,
    );
    let measured: Vec<crate::host_load::Measured> = probed
        .iter()
        .zip(scanned.scans.iter().map(|(_, scan)| scan))
        .map(|((host, probe), scan)| crate::host_load::Measured {
            host: host.clone(),
            probe: probe.clone(),
            idle: scan
                .as_ref()
                .map_err(String::clone)
                .and_then(|scan| scan.cpu_idle.clone()),
        })
        .collect();
    health.findings.extend(
        crate::host_load::findings(&measured, ready, tool.rules.floor_kib)
            .into_iter()
            .map(|advice| Finding::new(FindingKind::Load, advice)),
    );
    health.findings.extend(
        crate::proc_limit::findings(&probed, &busy)
            .into_iter()
            .map(|advice| Finding::new(FindingKind::ProcessLimit, advice)),
    );
    health.findings.extend(
        crate::host_load::builds_items_ready(&loaded)
            .map(|advice| Finding::new(FindingKind::Load, advice)),
    );
    // Read now: a probe in another command, or this one, may have just
    // written or removed one.
    let refusals = crate::host::refusals(&tool.places)?;
    let installed = crate::host_installed::read_all_installed(&tool.places, &ssh, &hosts);
    let (stopped, orphans) =
        crate::health_fix::stop_orphans(std::mem::take(&mut scanned.orphans), &|host, script| {
            crate::host::output_of(
                crate::host::command_at(&ssh, host, script, crate::host::Terminal::None),
                &format!("the stop script on {}", crate::host::place(&host.name)),
                crate::host::Accept::Zero,
            )
        });
    stopped.iter().for_each(crate::text::note);
    scanned.orphans = orphans;
    health
        .findings
        .extend(host_findings(&scanned, &busy, &refusals, &installed));
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
        // The dashboard, prune and the busy processes come before "needs
        // action", which is last.
        let mut out = lines(&health);
        let at = out
            .iter()
            .position(|line| line.starts_with("needs action"))
            .unwrap_or(out.len());
        let mut before = vec![
            match dashboard {
                Some(pid) => format!("dashboard: up (pid {pid})"),
                None => "dashboard: not running".to_string(),
            },
            prune_line,
        ];
        before.extend(crate::stuck::lines(&busy));
        if let Ok(caches) = &scanned.caches {
            before.extend(crate::sccache_queue::lines(caches));
        }
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
    use crate::side_git::SideRead;

    /// A build volume whose events macOS keeps a history of is a line under
    /// "needs action", and one it keeps none of adds none.
    #[test]
    fn a_volume_with_an_event_history_is_listed_under_needs_action() {
        let now = 1_800_000_000;
        let places = crate::testing::Scratch::new("health-event-log").places();
        let printed = |recorded: crate::fail::Result<bool>| {
            let mut health = assess(
                now,
                DiskSpace {
                    free_kib: 100 << 20,
                    floor_kib: 30 << 20,
                    hard_floor_kib: 10 << 20,
                },
                Machine {
                    at: now,
                    load: [1.0; 3],
                    cores: 8,
                    builds: Vec::new(),
                    remote_runs: Vec::new(),
                    app_builds: Vec::new(),
                    landings: Vec::new(),
                    unread: Vec::new(),
                    commits: Default::default(),
                    sccache: SccacheHealth {
                        server: SccacheServer::Read {
                            executed: 1,
                            compared: None,
                        },
                        waiting: Vec::new(),
                    },
                    shells: Shells::default(),
                    locks: Vec::new(),
                    queues: Vec::new(),
                },
                &[],
                Vec::new(),
                LeftBehind::default(),
                Vec::new(),
            );
            health
                .findings
                .extend(crate::fsevents_log::finding_of(&places, recorded));
            lines(&health)
        };
        let listed = printed(Ok(true));
        let at = listed
            .iter()
            .position(|line| line == "needs action:")
            .expect("health lists what needs action");
        assert!(
            listed[at + 1].contains("fsevents: macOS keeps a history of file events")
                && listed[at + 1].contains("sudo touch"),
            "{listed:?}"
        );
        assert!(
            printed(Ok(false))
                .iter()
                .any(|line| line == "needs action: nothing")
        );
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
            landings: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                server: SccacheServer::Read {
                    executed: 1,
                    compared: None,
                },
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
                note: Some(
                    crate::claim::Note::with_state(state, "kept for later".to_string()).unwrap(),
                ),
            },
            branch: Some("work".to_string()),
            git: Ok(SideRead {
                head: None,
                ahead: Vec::new(),
                main: "0".repeat(40),
                uncommitted: 3,
                last_write: Some(now - 3 * 3600),
            }),
            awaiting_review: n == 3,
            landing: None,
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

    /// An idle claim's finding says what blocks its branch from landing, and
    /// suggests the landing command when nothing does.
    #[test]
    fn an_idle_claims_finding_says_what_blocks_its_landing() {
        let now = 1_800_000_000;
        let machine = Machine {
            at: now,
            load: [1.0; 3],
            cores: 8,
            builds: Vec::new(),
            remote_runs: Vec::new(),
            app_builds: Vec::new(),
            landings: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                server: SccacheServer::Read {
                    executed: 1,
                    compared: None,
                },
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
        let claimed = |n: usize, landing: Advice| Claimed {
            place: n - 1,
            claim: Claim {
                who: format!("agent {n}"),
                since: now - 70 * 24 * 3600,
                note: None,
            },
            branch: Some("work".to_string()),
            git: Ok(SideRead {
                head: None,
                ahead: Vec::new(),
                main: "0".repeat(40),
                uncommitted: 3,
                last_write: Some(now - 3 * 3600),
            }),
            awaiting_review: false,
            landing: Some(landing),
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
            vec![
                claimed(
                    1,
                    Advice::run(
                        "nothing blocks landing: run".to_string(),
                        "builds land /demo-worktrees/side-1 --done 7 abc".to_string(),
                    ),
                ),
                claimed(
                    2,
                    Advice::said("its branch cannot land yet: finding 5 is open".to_string()),
                ),
            ],
            LeftBehind::default(),
            Vec::new(),
        );
        let lines: Vec<String> = health.findings.iter().map(|f| f.advice.line()).collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].ends_with(
                "no build runs there; nothing blocks landing: run `builds land /demo-worktrees/side-1 --done 7 abc`"
            ),
            "{lines:?}"
        );
        assert!(
            lines[1]
                .ends_with("no build runs there; its branch cannot land yet: finding 5 is open"),
            "{lines:?}"
        );
    }

    /// The coordinator's schedule can stop when every worktree with waiting
    /// commits is held and no agent runs; it is needed while an agent runs or
    /// commits wait to land with no hold.
    #[test]
    fn the_schedule_can_stop_only_when_every_claim_is_held() {
        let now = 1_800_000_000;
        let commit = |n: u8| Commit {
            full: format!("{n:040}"),
            sha: format!("{n:07}"),
            time: now - 3600,
            authored: now - 3600,
            subject: "work".to_string(),
        };
        let run = |states: &[Option<&str>],
                   ahead: usize,
                   unclaimed_ahead: usize,
                   claimed_unread: bool,
                   unclaimed_unread: bool| {
            let machine = Machine {
                at: now,
                load: [1.0; 3],
                cores: 8,
                builds: Vec::new(),
                remote_runs: Vec::new(),
                app_builds: Vec::new(),
                landings: Vec::new(),
                unread: Vec::new(),
                commits: Default::default(),
                sccache: SccacheHealth {
                    server: SccacheServer::Read {
                        executed: 1,
                        compared: None,
                    },
                    waiting: Vec::new(),
                },
                shells: Shells::default(),
                locks: Vec::new(),
                queues: Vec::new(),
            };
            let places: Vec<Place> = (1..=states.len() + 1)
                .map(|n| Place {
                    label: format!("demo/side-{n}"),
                    repo: "demo".to_string(),
                    top: PathBuf::from(format!("/demo-worktrees/side-{n}")),
                    root: PathBuf::from(format!("/volume/demo/side-{n}")),
                })
                .collect();
            let git = |ahead: usize, unread: bool| match unread {
                true => Err("git could not read it".to_string()),
                false => Ok(SideRead {
                    head: None,
                    ahead: (0..ahead).map(|n| commit(n as u8)).collect(),
                    main: "0".repeat(40),
                    uncommitted: 0,
                    last_write: Some(now - 60),
                }),
            };
            let claimed = states
                .iter()
                .enumerate()
                .map(|(at, state)| Claimed {
                    place: at,
                    claim: Claim {
                        who: format!("agent {at}"),
                        since: now - 60,
                        note: state.map(|state| {
                            crate::claim::Note::with_state(Some(state), "noted".to_string())
                                .unwrap()
                        }),
                    },
                    branch: Some("work".to_string()),
                    git: git(ahead, claimed_unread),
                    awaiting_review: false,
                    landing: None,
                })
                .collect();
            let left = LeftBehind {
                worktrees: vec![Unclaimed {
                    place: states.len(),
                    git: git(unclaimed_ahead, unclaimed_unread),
                    kept_unknown: None,
                }],
                branches: Vec::new(),
            };
            let health = assess(
                now,
                DiskSpace {
                    free_kib: 100 << 20,
                    floor_kib: 30 << 20,
                    hard_floor_kib: 10 << 20,
                },
                machine,
                &places,
                claimed,
                left,
                Vec::new(),
            );
            let printed = lines(&health);
            let last = printed.last().expect("health prints lines").clone();
            assert_eq!(health.schedule_needed, last == "health schedule: needed");
            health.schedule_needed
        };
        let needed = |states: &[Option<&str>], ahead, unclaimed_ahead| {
            run(states, ahead, unclaimed_ahead, false, false)
        };
        let needed_with_error =
            |states: &[Option<&str>], claimed, unclaimed| run(states, 0, 0, claimed, unclaimed);
        let held = Some("held");
        assert!(!needed(&[], 0, 0), "nothing runs");
        assert!(
            !needed(&[held, held], 2, 0),
            "held commits wait on a record"
        );
        assert!(needed(&[held, None], 2, 0), "an agent runs in one worktree");
        assert!(needed(&[None], 0, 0), "an agent runs with no commits yet");
        assert!(needed(&[held], 1, 1), "an unclaimed worktree has commits");
        assert!(
            needed_with_error(&[held], true, false),
            "git could not read a held worktree"
        );
        assert!(
            needed_with_error(&[], false, true),
            "git could not read an unclaimed worktree"
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
            landings: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                server: SccacheServer::Read {
                    executed: 1,
                    compared: None,
                },
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
            },
            branch: Some("work".to_string()),
            git: Ok(SideRead {
                head: None,
                ahead: Vec::new(),
                main: "0".repeat(40),
                uncommitted: 3,
                last_write: Some(now - changed_ago),
            }),
            awaiting_review: false,
            landing: None,
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
                command: "cargo".to_string(),
            }],
            app_builds: Vec::new(),
            landings: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                server: SccacheServer::Read {
                    executed: 1,
                    compared: None,
                },
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
            },
            branch: Some("work".to_string()),
            git: Ok(SideRead {
                head: None,
                ahead: Vec::new(),
                main: "0".repeat(40),
                uncommitted: 3,
                last_write: Some(now - 2 * 3600),
            }),
            awaiting_review: false,
            landing: None,
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
                landings: Vec::new(),
                unread: Vec::new(),
                commits: Default::default(),
                sccache: SccacheHealth {
                    server: SccacheServer::Read {
                        executed: 1,
                        compared: None,
                    },
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
                },
                branch: Some("work".to_string()),
                git: Ok(SideRead {
                    head: None,
                    ahead: Vec::new(),
                    main: "0".repeat(40),
                    uncommitted: 0,
                    last_write: Some(now - 2 * 3600),
                }),
                awaiting_review: false,
                landing: None,
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

    /// A claimed worktree with no change for hours has work running while a
    /// `builds land` runs there, including one that waits for the landing of
    /// another worktree: health prints no land command for it and says what
    /// the landing waits for, and the dashboard's stalled mark follows.
    #[test]
    fn a_quiet_worktree_whose_landing_waits_for_another_landing_is_not_idle() {
        let now = 1_800_000_000;
        let judge = |landings: Vec<LandingRun>| {
            let machine = Machine {
                at: now,
                load: [1.0; 3],
                cores: 8,
                builds: Vec::new(),
                remote_runs: Vec::new(),
                app_builds: Vec::new(),
                landings,
                unread: Vec::new(),
                commits: Default::default(),
                sccache: SccacheHealth {
                    server: SccacheServer::Read {
                        executed: 1,
                        compared: None,
                    },
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
                },
                branch: Some("work".to_string()),
                git: Ok(SideRead {
                    head: None,
                    ahead: Vec::new(),
                    main: "0".repeat(40),
                    uncommitted: 0,
                    last_write: Some(now - 2 * 3600),
                }),
                awaiting_review: false,
                landing: Some(Advice::run(
                    "nothing blocks landing: run".to_string(),
                    "builds land /demo-worktrees/side-1 --done 7 abc".to_string(),
                )),
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
        let waiting = judge(vec![LandingRun {
            pid: 5151,
            place: 0,
            waits: LandingWait::For(
                "the landing that changes the apps, process 36208 landing --done 1276 abc"
                    .to_string(),
            ),
        }]);
        assert!(waiting.findings.is_empty(), "{:?}", waiting.findings);
        assert!(!waiting.worktrees[0].stalled);
        let shown = lines(&waiting);
        assert!(
            shown.iter().any(|line| line.contains(
                "builds land waits for another landing (pid 5151; the landing that changes the apps, process 36208 landing --done 1276 abc)"
            )),
            "{shown:?}"
        );
        assert!(
            shown.iter().all(|line| !line.contains("run `builds land")),
            "{shown:?}"
        );
        let running = judge(vec![LandingRun {
            pid: 5151,
            place: 0,
            waits: LandingWait::Nothing,
        }]);
        assert!(running.findings.is_empty(), "{:?}", running.findings);
        assert!(
            lines(&running)
                .iter()
                .any(|line| line.contains("builds land runs (pid 5151)"))
        );
        let none = judge(Vec::new());
        assert_eq!(none.findings.len(), 1);
        assert!(none.worktrees[0].stalled);
    }

    /// A claimed worktree with no change for hours is building while an app
    /// build of one of its commits waits in a lane's queue, wherever the
    /// build was run from, and health shows its place in line; a queued build
    /// of another commit leaves it idle.
    #[test]
    fn a_quiet_worktree_whose_commit_waits_in_a_lane_queue_is_not_idle() {
        let now = 1_800_000_000;
        let commit = |full: &str| Commit {
            sha: full[..7].to_string(),
            full: full.to_string(),
            time: now - 2 * 3600,
            authored: now - 2 * 3600,
            subject: "Work".to_string(),
        };
        let mine = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let queued = |pid: i32, commit: &str| Queued {
            pid,
            minutes: 3,
            command: format!("builds app-build demo --lane android {commit}"),
            started_from: Some("demo/main".to_string()),
            commit: Some(commit.to_string()),
        };
        let judge = |queue: Vec<Queued>| {
            let machine = Machine {
                at: now,
                load: [1.0; 3],
                cores: 8,
                builds: Vec::new(),
                remote_runs: Vec::new(),
                app_builds: Vec::new(),
                landings: Vec::new(),
                unread: Vec::new(),
                commits: Default::default(),
                sccache: SccacheHealth {
                    server: SccacheServer::Read {
                        executed: 1,
                        compared: None,
                    },
                    waiting: Vec::new(),
                },
                shells: Shells::default(),
                locks: Vec::new(),
                queues: vec![LaneQueue {
                    repo: "demo".to_string(),
                    lane: Lane::Android,
                    queue,
                }],
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
                },
                branch: Some("work".to_string()),
                git: Ok(SideRead {
                    head: Some(commit(mine)),
                    ahead: vec![commit(mine)],
                    main: "0".repeat(40),
                    uncommitted: 0,
                    last_write: None,
                }),
                awaiting_review: false,
                landing: None,
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
        let other = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let waiting = judge(vec![queued(61, other), queued(62, mine)]);
        assert!(waiting.findings.is_empty(), "{:?}", waiting.findings);
        let shown = lines(&waiting);
        assert!(
            shown.iter().any(|line| line.ends_with(
                "; app build waiting in demo's android lane, 2nd of 2 in line (pid 62)"
            )),
            "{shown:?}"
        );
        assert_eq!(judge(vec![queued(61, other)]).findings.len(), 1);
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
            landings: Vec::new(),
            unread: vec![line.to_string()],
            commits: Default::default(),
            sccache: SccacheHealth {
                server: SccacheServer::Read {
                    executed: 1,
                    compared: None,
                },
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

    /// A server whose stats did not read says why in its line, and, while
    /// clients wait on it, in a finding.
    #[test]
    fn a_server_whose_stats_did_not_read_says_why() {
        let now = 1_800_000_000;
        let error = "sccache --show-stats exited 2 and said: no server";
        let oldest = CLIENT_MINUTES + 3;
        let machine = Machine {
            at: now,
            load: [1.0; 3],
            cores: 8,
            builds: Vec::new(),
            remote_runs: Vec::new(),
            app_builds: Vec::new(),
            landings: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                server: SccacheServer::Unread(error.to_string()),
                waiting: vec![Client {
                    pid: 41,
                    minutes: oldest,
                    cpu_seconds: 0.1,
                    checkout: None,
                }],
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
                FindingKind::Sccache,
                Advice::said(format!(
                    "sccache: 1 clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m), and health cannot tell whether the server finishes compiles, because reading its stats failed: {error}"
                ))
            )]
        );
        let lines = lines(&health);
        assert!(
            lines.contains(&format!(
                "sccache: reading the server's stats failed: {error}; 1 clients waiting {CLIENT_MINUTES}+ min"
            )),
            "{lines:?}"
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
            landings: Vec::new(),
            unread: Vec::new(),
            commits: Default::default(),
            sccache: SccacheHealth {
                server: SccacheServer::Read {
                    executed: 1,
                    compared: None,
                },
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
        let git = |ahead: Vec<Commit>| {
            Ok(SideRead {
                head: None,
                ahead,
                main: "0".repeat(40),
                uncommitted: 0,
                last_write: None,
            })
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
                        kept_unknown: Some("project demo's lists: broken".to_string()),
                    },
                    Unclaimed {
                        place: 1,
                        git: git(Vec::new()),
                        kept_unknown: None,
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
                "builds cannot tell whether an item is kept there, since project demo's lists: broken"
            ),
            "{:?}",
            health.findings
        );
    }

    /// A reading of the hosts that found nothing.
    fn nothing_scanned() -> Scanned {
        Scanned {
            scans: Vec::new(),
            install: Ok(None),
            orphans: Vec::new(),
            caches: Ok(Vec::new()),
            now: 0,
        }
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
        let found = host_findings(&nothing_scanned(), &busy, &[refusal], &[]);
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
        let found = host_findings(&nothing_scanned(), &[], &[refusal], &[]);
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
        let found = host_findings(&nothing_scanned(), &[], &[refusal], &[]);
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
