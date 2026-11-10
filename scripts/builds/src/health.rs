//! `builds health`: what on this machine is stuck or needs acting on, for a
//! coordinator that checks every few minutes. It reads; the one thing it
//! keeps is its own record of sccache's finished compiles, to tell next time
//! whether the server finished any since.
//!
//! It exits 1 when something needs acting on: clients waiting on an sccache
//! server that finished nothing since the last readings, a Claude shell over
//! two hours old, a claimed worktree with no change and no build for an hour
//! (its own, or one it holds the app lock for), or the volume under its
//! free-space floor.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::checkout::{Root, parallel_map};
use crate::claim::Claim;
use crate::fail::{Context, Fail, Result};
use crate::procs::{
    BUILD_NAMES, BuildProcess, Place, Process, Processes, args, build_processes, cores, cwd, load_average, open_files,
    usage,
};
use crate::side_git::SideGit;
use crate::text::{human, local_minute};
use crate::tool::Tool;

/// A Claude Code shell older than this is a stale watcher or loop.
const SHELL_MINUTES: i64 = 120;
/// An sccache client waiting this long is worth a look.
const CLIENT_MINUTES: i64 = 10;
/// A claimed worktree with no change and no build this long has stalled.
const QUIET_MINUTES: i64 = 60;
/// Readings of sccache's count this old or older say whether it moved.
const READING_MINUTES: i64 = 5;

#[derive(Clone, Serialize, Deserialize)]
pub struct Health {
    pub generated_at: i64,
    pub load: [f64; 3],
    pub cores: usize,
    pub free_kib: u64,
    pub floor_kib: u64,
    pub hard_floor_kib: u64,
    pub sccache: SccacheHealth,
    pub shells: Vec<Shell>,
    pub worktrees: Vec<WorktreeHealth>,
    pub locks: Vec<LockHealth>,
    /// What needs acting on, one line each; empty when nothing does.
    pub findings: Vec<String>,
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
pub struct Shell {
    pub pid: i32,
    pub minutes: i64,
    pub command: String,
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
    pub building: Vec<BuildProcess>,
    /// The holder of an app lock building for this worktree in the app
    /// checkout (`Holder::builds_for`).
    pub app_build: Option<i32>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LockHealth {
    pub repo: String,
    pub path: String,
    pub holder: Option<Holder>,
    pub waiting: usize,
    /// Those waiting, next first.
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
}

impl Holder {
    /// Does it build for the worktree <label> at <git>? When it was run from
    /// there, or the app checkout is at a commit of that worktree's own (one
    /// origin/main has no copy of), as when it was run from elsewhere or its
    /// `builds lock` was killed and a process of its build holds the lock.
    fn builds_for(&self, label: &str, git: &SideGit) -> bool {
        self.started_from.as_deref() == Some(label)
            || (self.app_head.is_some() && git.ahead.iter().any(|c| Some(&c.full) == self.app_head.as_ref()))
    }
}

/// A repo's app lock and its app checkout.
pub struct AppLock {
    pub repo: String,
    pub path: PathBuf,
    /// The app checkout's top, when it is a live checkout.
    pub app_top: Option<PathBuf>,
}

/// The record of sccache readings between runs.
fn readings_file(home: &Path) -> PathBuf {
    home.join("Library/Caches/builds/sccache-readings.json")
}

fn read_readings(file: &Path) -> Result<Vec<Reading>> {
    match std::fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes).context(|| format!("reading {}", file.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(Fail::Message(format!("reading {}: {error}", file.display()))),
    }
}

fn write_readings(file: &Path, readings: &[Reading]) -> Result<()> {
    let dir = file.parent().expect("the file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let staged = file.with_extension(format!("{}", std::process::id()));
    std::fs::write(&staged, serde_json::to_vec(readings).expect("readings serialize"))
        .context(|| format!("writing {}", staged.display()))?;
    std::fs::rename(&staged, file).context(|| format!("writing {}", file.display()))
}

fn minutes_since(now: i64, then: i64) -> i64 {
    (now - then).max(0) / 60
}

/// The command a Claude Code shell runs: what its `eval '...'` holds.
fn shell_command(argv: &[String]) -> String {
    let line = argv.join(" ");
    let inner = match line.find("eval '") {
        Some(at) => {
            let rest = &line[at + "eval '".len()..];
            let end = rest.rfind("' && pwd -P").or_else(|| rest.rfind("' < /dev/null")).unwrap_or(rest.len());
            rest[..end].replace("'\"'\"'", "'")
        }
        None => line,
    };
    let flat: String = inner.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 200 { format!("{}…", flat.chars().take(200).collect::<String>()) } else { flat }
}

/// Is <command> `builds dashboard`, which runs until it is stopped on
/// purpose?
fn runs_the_dashboard(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words.windows(2).next().is_some_and(|pair| (pair[0] == "builds" || pair[0].ends_with("/builds")) && pair[1] == "dashboard")
}

fn command_line(pid: i32) -> String {
    let line = args(pid).unwrap_or_default().join(" ");
    if line.chars().count() > 200 { format!("{}…", line.chars().take(200).collect::<String>()) } else { line }
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
    pub sccache: SccacheHealth,
    pub shells: Vec<Shell>,
    pub locks: Vec<LockHealth>,
}

/// sccache's count of finished compiles now, against the newest reading at
/// least READING_MINUTES old; this reading joins the record when the newest
/// in it is a minute old or more.
pub fn sccache_reading(home: &Path, now: i64) -> Result<(bool, Option<u64>, Option<Reading>)> {
    let listening = crate::sccache::listening();
    let executed = if listening {
        crate::sccache::count(&crate::sccache::stats(), "Compile requests executed")
    } else {
        None
    };
    let file = readings_file(home);
    let mut readings = read_readings(&file)?;
    let compared = readings.iter().filter(|r| now - r.at >= READING_MINUTES * 60).max_by_key(|r| r.at).cloned();
    if let Some(executed) = executed
        && readings.iter().all(|r| now - r.at >= 60) {
            readings.push(Reading { executed, at: now });
            // Keep what the next runs compare against: the last hour.
            readings.retain(|r| now - r.at <= 3600);
            write_readings(&file, &readings)?;
        }
    Ok((listening, executed, compared))
}

/// Reads the processes now: build processes placed in <places>, sccache's
/// waiting clients, Claude shells, and the holders of <locks> (each repo's
/// app.lock). <sccache> is a `sccache_reading`.
pub fn sample(
    places: &[Place],
    locks: &[AppLock],
    uid: u32,
    now: i64,
    sccache: (bool, Option<u64>, Option<Reading>),
) -> Machine {
    let processes = Processes::read();
    let builds = build_processes(&processes, places, &BUILD_NAMES);
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
        let checkout = builds.iter().find(|b| b.pid == process.pid).map(|b| places[b.place].label.clone());
        waiting.push(Client {
            pid: process.pid,
            minutes: minutes_since(now, process.started),
            cpu_seconds: usage(process.pid).map(|(cpu, _)| cpu).unwrap_or(0.0),
            checkout,
        });
    }
    waiting.sort_by_key(|c| std::cmp::Reverse(c.minutes));
    let mut shells = Vec::new();
    for process in &processes.list {
        if !matches!(process.name.as_str(), "zsh" | "bash" | "sh") || process.uid != uid {
            continue;
        }
        let minutes = minutes_since(now, process.started);
        if minutes < SHELL_MINUTES {
            continue;
        }
        let Some(argv) = args(process.pid) else { continue };
        if !argv.iter().any(|arg| arg.contains("shell-snapshots")) {
            continue;
        }
        let command = shell_command(&argv);
        if !runs_the_dashboard(&command) {
            shells.push(Shell { pid: process.pid, minutes, command });
        }
    }
    let mut held = Vec::new();
    for AppLock { repo, path, app_top } in locks {
        let shown = path.display().to_string();
        let real = std::fs::canonicalize(path).unwrap_or(path.clone());
        let opened = |pid: i32| open_files(pid).iter().any(|open| *open == real || open == path);
        let mut holding = None;
        let mut waiting = 0;
        // `builds lock <repo> ...` and `builds build-app <repo> ...`: the one
        // with the lock's file open holds it; the others wait in its queue.
        for process in processes.list.iter().filter(|p| p.name == "builds") {
            let locking = |argv: Vec<String>| {
                argv.get(1).is_some_and(|a| a == "lock" || a == "build-app") && argv.get(2) == Some(repo)
            };
            if !args(process.pid).is_some_and(locking) {
                continue;
            }
            if opened(process.pid) {
                holding = Some(process);
            } else {
                waiting += 1;
            }
        }
        // A build whose `builds lock` was killed outright still holds the
        // lock: its processes have the file open. Found only when the lock is
        // taken, since reading every process's open files costs.
        if holding.is_none() && taken(path) {
            holding = topmost(&processes, uid, opened);
        }
        let holder = holding.map(|process| Holder {
            pid: process.pid,
            minutes: minutes_since(now, process.started),
            command: command_line(process.pid),
            running: processes.children(process.pid).map(|child| command_line(child.pid)).collect(),
            started_from: cwd(process.pid).and_then(|dir| place_of(places, &dir)).map(|at| places[at].label.clone()),
            app_head: app_top.as_deref().and_then(head_of),
        });
        let holder_pid = holder.as_ref().map(|h| h.pid);
        let in_line = path.parent().map(|repo_dir| crate::queue::Queue::in_repo_dir(repo_dir).waiting()).unwrap_or_default();
        let queue = in_line
            .into_iter()
            .filter(|pid| Some(*pid) != holder_pid)
            .filter_map(|pid| processes.list.iter().find(|p| p.pid == pid && p.name == "builds"))
            .map(|process| Queued {
                pid: process.pid,
                minutes: minutes_since(now, process.started),
                command: command_line(process.pid),
                started_from: cwd(process.pid).and_then(|dir| place_of(places, &dir)).map(|at| places[at].label.clone()),
            })
            .collect();
        held.push(LockHealth { repo: repo.clone(), path: shown, holder, waiting, queue });
    }
    Machine {
        at: now,
        load: load_average(),
        cores: cores(),
        builds,
        sccache: SccacheHealth { listening, executed, compared, waiting },
        shells,
        locks: held,
    }
}

/// Is the lock at <path> taken? Asked by taking it without waiting and
/// letting it go at once.
fn taken(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_EXLOCK | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd >= 0 {
        unsafe { libc::close(fd) };
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

/// Of <uid>'s processes for which <holds> is true, the oldest one whose
/// parent doesn't hold it too: the top of the build holding a lock.
fn topmost(processes: &Processes, uid: u32, holds: impl Fn(i32) -> bool) -> Option<&Process> {
    let holding: Vec<&Process> = processes.list.iter().filter(|p| p.uid == uid && holds(p.pid)).collect();
    holding
        .iter()
        .filter(|p| !holding.iter().any(|q| q.pid == p.ppid))
        .min_by_key(|p| p.started)
        .copied()
}

/// The place <dir> is in, the deepest when several hold it.
fn place_of(places: &[Place], dir: &Path) -> Option<usize> {
    (0..places.len()).filter(|at| dir.starts_with(&places[*at].top)).max_by_key(|at| places[*at].top.as_os_str().len())
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
}

/// A side worktree with no claim that holds commits missing from origin/main:
/// work an agent left behind.
pub struct Unclaimed {
    pub place: usize,
    pub git: SideGit,
}

/// Judges what needs acting on.
pub fn assess(
    now: i64,
    free_kib: u64,
    floor_kib: u64,
    hard_floor_kib: u64,
    machine: Machine,
    places: &[Place],
    claimed: Vec<Claimed>,
    unclaimed: Vec<Unclaimed>,
) -> Health {
    let mut findings = Vec::new();
    if free_kib < floor_kib {
        findings.push(format!("disk: {} free, under the {} floor", human(free_kib), human(floor_kib)));
    }
    let s = &machine.sccache;
    if let Some(oldest) = s.waiting.first().map(|c| c.minutes) {
        match (s.listening, s.executed, &s.compared) {
            (false, _, _) => findings.push(format!(
                "sccache: no server is listening and {} clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m)",
                s.waiting.len()
            )),
            (true, Some(now_count), Some(before)) if now_count == before.executed => findings.push(format!(
                "sccache: {} clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m) and the server finished no compile since {} ({}m ago)",
                s.waiting.len(),
                local_minute(before.at),
                minutes_since(now, before.at)
            )),
            _ => {}
        }
    }
    for shell in &machine.shells {
        findings.push(format!(
            "shell: pid {} has run {}h{:02}m: {}",
            shell.pid,
            shell.minutes / 60,
            shell.minutes % 60,
            shell.command
        ));
    }
    for Unclaimed { place, git } in unclaimed {
        if !git.ahead.is_empty() {
            findings.push(format!(
                "worktree: {} has no claim and {} waiting to land, so it is not retired",
                places[place].label,
                crate::text::counted(git.ahead.len(), "commit", "commits")
            ));
        }
    }
    let mut worktrees = Vec::new();
    for Claimed { place, claim, branch, git } in claimed {
        let building: Vec<BuildProcess> = machine.builds.iter().filter(|b| b.place == place).cloned().collect();
        let quiet_minutes = git.activity().map(|when| minutes_since(now, when));
        let label = places[place].label.clone();
        let app_build =
            machine.locks.iter().filter_map(|lock| lock.holder.as_ref()).find(|h| h.builds_for(&label, &git)).map(|h| h.pid);
        // A held claim is quiet on purpose: its note says why.
        let held = claim.note_state.as_deref() == Some("held");
        if !held && building.is_empty() && app_build.is_none() && quiet_minutes.is_none_or(|m| m >= QUIET_MINUTES) {
            findings.push(format!(
                "worktree: {label} ({}) has had no change for {} and no build runs there",
                claim.who,
                quiet_minutes.map(|m| format!("{m}m")).unwrap_or_else(|| "as long as git can tell".to_string())
            ));
        }
        worktrees.push(WorktreeHealth {
            checkout: label,
            path: places[place].top.display().to_string(),
            claim,
            branch,
            git,
            quiet_minutes,
            building,
            app_build,
        });
    }
    Health {
        generated_at: now,
        load: machine.load,
        cores: machine.cores,
        free_kib,
        floor_kib,
        hard_floor_kib,
        sccache: machine.sccache,
        shells: machine.shells,
        worktrees,
        locks: machine.locks,
        findings,
    }
}

/// The places processes are placed in: every live checkout.
pub fn places_of(roots: &[Root]) -> Vec<Place> {
    roots
        .iter()
        .filter(|root| root.live.is_some())
        .map(|root| Place { label: format!("{}/{}", root_repo(root), root.name()), top: root.top.clone(), root: root.path.clone() })
        .collect()
}

/// Each repo's app lock that exists, with its app checkout.
pub fn locks_of(places: &crate::places::Places, roots: &[Root]) -> Result<Vec<AppLock>> {
    let mut repos: Vec<String> = roots.iter().map(root_repo).collect();
    repos.dedup();
    let mut locks = Vec::new();
    for repo in repos {
        let path = places.root.join(&repo).join("app.lock");
        if !path.is_file() {
            continue;
        }
        let app = places.app_checkout_of(&repo)?;
        let app_top = roots
            .iter()
            .find(|root| root_repo(root) == repo && Some(root.name()) == app && root.live.is_some())
            .map(|root| root.top.clone());
        locks.push(AppLock { repo, path, app_top });
    }
    Ok(locks)
}

/// Gathers the machine's health now, reading everything itself.
pub fn health(tool: &Tool) -> Result<Health> {
    let now = tool.now;
    let roots = tool.roots()?;
    let live: Vec<&Root> = roots.iter().filter(|root| root.live.is_some()).collect();
    let places = places_of(&roots);
    let sccache = sccache_reading(&tool.places.home, now)?;
    let machine = sample(&places, &locks_of(&tool.places, &roots)?, tool.places.uid, now, sccache);
    let mut claimed = Vec::new();
    let mut unclaimed = Vec::new();
    for (at, root) in live.iter().enumerate() {
        let checkout = root.live.as_ref().expect("live");
        if checkout.is_main() {
            continue;
        }
        match Claim::read(&checkout.claim_file())? {
            Some(claim) => claimed.push((at, checkout, claim)),
            None => unclaimed.push((at, checkout)),
        }
    }
    let unclaimed_gits = parallel_map(&unclaimed, |(_, checkout)| crate::side_git::read(&checkout.top));
    let unclaimed: Vec<Unclaimed> =
        unclaimed.into_iter().zip(unclaimed_gits).map(|((place, _), git)| Unclaimed { place, git }).collect();
    let gits = parallel_map(&claimed, |(_, checkout, _)| crate::side_git::read(&checkout.top));
    let mut judged = Vec::new();
    for ((place, checkout, claim), git) in claimed.into_iter().zip(gits) {
        judged.push(Claimed { place, claim, branch: checkout.branch()?, git });
    }
    Ok(assess(now, tool.free_kib()?, tool.rules.floor_kib, tool.rules.hard_floor_kib, machine, &places, judged, unclaimed))
}

fn root_repo(root: &Root) -> String {
    root.path.parent().map(crate::checkout::file_name).unwrap_or_default()
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

/// The lines `builds health` prints.
pub fn lines(health: &Health) -> Vec<String> {
    let mut out = Vec::new();
    let [one, five, fifteen] = health.load;
    out.push(format!("load: {one:.1} {five:.1} {fifteen:.1} on {} cores", health.cores));
    out.push(format!(
        "disk: {} free; warns under {}, stops under {}",
        human(health.free_kib),
        human(health.floor_kib),
        human(health.hard_floor_kib)
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
            None => format!("sccache: {executed} compiles executed; no reading {READING_MINUTES}+ min old to compare with yet"),
        },
    };
    if s.waiting.is_empty() {
        line += &format!("; no client waiting {CLIENT_MINUTES}+ min");
    } else {
        line += &format!("; {} clients waiting {CLIENT_MINUTES}+ min", s.waiting.len());
    }
    out.push(line);
    for client in &s.waiting {
        out.push(format!(
            "    pid {} waiting {}m, {:.1}s CPU{}",
            client.pid,
            client.minutes,
            client.cpu_seconds,
            client.checkout.as_ref().map(|c| format!(", {c}")).unwrap_or_default()
        ));
    }
    out.push(format!(
        "shells: {} older than {}h",
        crate::text::counted(health.shells.len(), "Claude shell", "Claude shells"),
        SHELL_MINUTES / 60
    ));
    for shell in &health.shells {
        out.push(format!("    pid {} {}h{:02}m: {}", shell.pid, shell.minutes / 60, shell.minutes % 60, shell.command));
    }
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
            w.claim.since,
            match (&w.claim.note_state, &w.claim.note) {
                (Some(state), note) => format!(" ({}): {}", state.replace('-', " "), note.as_deref().unwrap_or("")),
                (None, Some(note)) => format!(": {note}"),
                (None, None) => String::new(),
            }
        ));
        out.push(format!(
            "        {git}; last change {}; {}",
            w.quiet_minutes.map(|m| format!("{m}m ago")).unwrap_or_else(|| "unknown".to_string()),
            match (w.building.is_empty(), w.app_build) {
                (true, None) => "no build running".to_string(),
                (true, Some(pid)) => format!("building in the app checkout (pid {pid})"),
                (false, None) => format!("building: {}", process_summary(&w.building)),
                (false, Some(pid)) => format!("building: {}; and in the app checkout (pid {pid})", process_summary(&w.building)),
            }
        ));
    }
    for lock in &health.locks {
        match &lock.holder {
            Some(holder) => out.push(format!(
                "app lock {}: held {}m by pid {} ({}), {} waiting{}",
                lock.repo,
                holder.minutes,
                holder.pid,
                holder.command,
                lock.waiting,
                if holder.running.is_empty() { String::new() } else { format!("; running {}", holder.running.join("; ")) }
            )),
            None => out.push(format!("app lock {}: free, {} waiting", lock.repo, lock.waiting)),
        }
    }
    if health.findings.is_empty() {
        out.push("needs action: nothing".to_string());
    } else {
        out.push("needs action:".to_string());
        for finding in &health.findings {
            out.push(format!("    {finding}"));
        }
    }
    out
}

/// `builds health [--json]`: exits 1 when something needs acting on.
pub fn cmd_health(tool: &Tool, args: &[String]) -> Result<()> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => crate::fail!("usage: builds health [--json]"),
    };
    tool.places.require_volume()?;
    let health = match crate::daemon::client::health(&tool.places) {
        Some(health) => health,
        None => health(tool)?,
    };
    if json {
        println!("{}", serde_json::to_string(&health).expect("health serializes"));
    } else {
        for line in lines(&health) {
            println!("{line}");
        }
    }
    if health.findings.is_empty() { Ok(()) } else { Err(Fail::Exit(1)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::side_git::Commit;

    #[test]
    fn a_claude_shell_shows_its_command() {
        let argv: Vec<String> = [
            "/bin/zsh",
            "-c",
            "source /Users/x/.claude/shell-snapshots/snapshot-zsh-1.sh 2>/dev/null || true && eval 'until ! pgrep -f '\"'\"'cargo test'\"'\"'; do sleep 5; done' < /dev/null && pwd -P >| /tmp/claude-1-cwd",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(shell_command(&argv), "until ! pgrep -f 'cargo test'; do sleep 5; done");
        assert!(runs_the_dashboard("/Users/x/.local/bin/builds dashboard --port 7787"));
        assert!(!runs_the_dashboard("builds status"));
    }

    /// A claim with no change for hours and no build is idle, unless its
    /// note says it is held.
    #[test]
    fn a_held_claim_is_not_idle() {
        let now = 1_800_000_000;
        let machine = Machine {
            at: now,
            load: [1.0; 3],
            cores: 8,
            builds: Vec::new(),
            sccache: SccacheHealth { listening: true, executed: Some(1), compared: None, waiting: Vec::new() },
            shells: Vec::new(),
            locks: Vec::new(),
        };
        let place = |n: usize| Place {
            label: format!("demo/side-{n}"),
            top: PathBuf::from(format!("/demo-worktrees/side-{n}")),
            root: PathBuf::from(format!("/volume/demo/side-{n}")),
        };
        let claimed = |n: usize, state: Option<&str>| Claimed {
            place: n - 1,
            claim: Claim {
                who: format!("agent {n}"),
                since: "2026-11-05 08:00".to_string(),
                note: Some("kept for later".to_string()),
                note_state: state.map(str::to_string),
            },
            branch: Some("work".to_string()),
            git: SideGit { head: None, ahead: Vec::new(), uncommitted: Some(3), last_write: Some(now - 3 * 3600), error: None },
        };
        let health = assess(
            now,
            100 << 20,
            30 << 20,
            10 << 20,
            machine,
            &[place(1), place(2)],
            vec![claimed(1, None), claimed(2, Some("held"))],
            Vec::new(),
        );
        assert_eq!(health.findings.len(), 1, "{:?}", health.findings);
        assert!(health.findings[0].starts_with("worktree: demo/side-1 "), "{:?}", health.findings);
        assert_eq!(health.worktrees.len(), 2);
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
            sccache: SccacheHealth { listening: true, executed: Some(1), compared: None, waiting: Vec::new() },
            shells: Vec::new(),
            locks: Vec::new(),
        };
        let place = |n: usize| Place {
            label: format!("demo/side-{n}"),
            top: PathBuf::from(format!("/demo-worktrees/side-{n}")),
            root: PathBuf::from(format!("/volume/demo/side-{n}")),
        };
        let commit = Commit { sha: "abc".into(), full: "abc".into(), time: now, authored: now, subject: "Work".into() };
        let git = |ahead: Vec<Commit>| SideGit { head: None, ahead, uncommitted: Some(0), last_write: None, error: None };
        let health = assess(
            now,
            100 << 20,
            30 << 20,
            10 << 20,
            machine,
            &[place(1), place(2)],
            Vec::new(),
            vec![Unclaimed { place: 0, git: git(vec![commit]) }, Unclaimed { place: 1, git: git(Vec::new()) }],
        );
        assert_eq!(health.findings.len(), 1, "{:?}", health.findings);
        assert!(
            health.findings[0].starts_with("worktree: demo/side-1 has no claim and 1 commit waiting to land"),
            "{:?}",
            health.findings
        );
    }
}
