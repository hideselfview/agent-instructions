//! `builds health`: what on this machine is stuck or needs acting on, for a
//! coordinator that checks every few minutes. It reads; the one thing it
//! keeps is its own record of sccache's finished compiles, to tell next time
//! whether the server finished any since.
//!
//! It exits 1 when something needs acting on: clients waiting on an sccache
//! server that finished nothing since the last readings, a Claude shell over
//! two hours old, a claimed worktree with no change and no build for an hour,
//! or the volume under its free-space floor.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::checkout::{Root, parallel_map};
use crate::claim::Claim;
use crate::fail::{Context, Fail, Result};
use crate::procs::{BUILD_NAMES, BuildProcess, Place, Processes, args, build_processes, cores, load_average, open_files, usage};
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

#[derive(Serialize)]
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

#[derive(Serialize)]
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

#[derive(Serialize)]
pub struct Client {
    pub pid: i32,
    pub minutes: i64,
    pub cpu_seconds: f64,
    pub checkout: Option<String>,
}

#[derive(Serialize)]
pub struct Shell {
    pub pid: i32,
    pub minutes: i64,
    pub command: String,
}

#[derive(Serialize)]
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
}

#[derive(Serialize)]
pub struct LockHealth {
    pub repo: String,
    pub path: String,
    pub holder: Option<Holder>,
    pub waiting: usize,
}

#[derive(Serialize)]
pub struct Holder {
    pub pid: i32,
    pub minutes: i64,
    pub command: String,
    /// What it runs: its children's commands.
    pub running: Vec<String>,
}

/// The record of sccache readings between runs.
fn readings_file(tool: &Tool) -> PathBuf {
    tool.places.home.join("Library/Caches/builds/sccache-readings.json")
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

/// Gathers the machine's health now.
pub fn health(tool: &Tool) -> Result<Health> {
    let now = tool.now;
    let roots = tool.roots()?;
    let live: Vec<&Root> = roots.iter().filter(|root| root.live.is_some()).collect();
    let places: Vec<Place> = live
        .iter()
        .map(|root| Place { label: format!("{}/{}", root_repo(root), root.name()), top: root.top.clone(), root: root.path.clone() })
        .collect();
    let processes = Processes::read();
    let builds = build_processes(&processes, &places, &BUILD_NAMES);
    let mut findings = Vec::new();

    let free_kib = tool.free_kib()?;
    if free_kib < tool.rules.floor_kib {
        findings.push(format!(
            "disk: {} free, under the {} floor",
            human(free_kib),
            human(tool.rules.floor_kib)
        ));
    }

    // sccache.
    let listening = crate::sccache::listening();
    let executed = if listening {
        crate::sccache::count(&crate::sccache::stats(), "Compile requests executed")
    } else {
        None
    };
    let file = readings_file(tool);
    let mut readings = read_readings(&file)?;
    let compared = readings.iter().filter(|r| now - r.at >= READING_MINUTES * 60).max_by_key(|r| r.at).cloned();
    if let Some(executed) = executed {
        readings.push(Reading { executed, at: now });
        // Keep what the next runs compare against: the last hour.
        readings.retain(|r| now - r.at <= 3600);
        write_readings(&file, &readings)?;
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
        let checkout = builds.iter().find(|b| b.pid == process.pid).map(|b| places[b.place].label.clone());
        waiting.push(Client {
            pid: process.pid,
            minutes: minutes_since(now, process.started),
            cpu_seconds: usage(process.pid).map(|(cpu, _)| cpu).unwrap_or(0.0),
            checkout,
        });
    }
    waiting.sort_by_key(|c| std::cmp::Reverse(c.minutes));
    if !waiting.is_empty() {
        let oldest = waiting[0].minutes;
        match (listening, executed, &compared) {
            (false, _, _) => findings.push(format!(
                "sccache: no server is listening and {} clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m)",
                waiting.len()
            )),
            (true, Some(now_count), Some(before)) if now_count == before.executed => findings.push(format!(
                "sccache: {} clients have waited {CLIENT_MINUTES}+ min (oldest {oldest}m) and the server finished no compile since {} ({}m ago)",
                waiting.len(),
                local_minute(before.at),
                minutes_since(now, before.at)
            )),
            _ => {}
        }
    }

    // Claude Code shells.
    let mut shells = Vec::new();
    for process in &processes.list {
        if !matches!(process.name.as_str(), "zsh" | "bash" | "sh") || process.uid != tool.places.uid {
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
        if runs_the_dashboard(&command) {
            continue;
        }
        findings.push(format!("shell: pid {} has run {}h{:02}m: {command}", process.pid, minutes / 60, minutes % 60));
        shells.push(Shell { pid: process.pid, minutes, command });
    }

    // Claimed side worktrees.
    let mut claimed = Vec::new();
    for (at, root) in live.iter().enumerate() {
        let checkout = root.live.as_ref().expect("live");
        if checkout.is_main() {
            continue;
        }
        if let Some(claim) = Claim::read(&checkout.claim_file())? {
            claimed.push((at, *root, claim));
        }
    }
    let gits = parallel_map(&claimed, |(_, root, _)| crate::side_git::read(&root.top));
    let mut worktrees = Vec::new();
    for ((at, root, claim), git) in claimed.into_iter().zip(gits) {
        let checkout = root.live.as_ref().expect("live");
        let building: Vec<BuildProcess> = builds.iter().filter(|b| b.place == at).cloned().collect();
        let quiet_minutes = git.activity().map(|when| minutes_since(now, when));
        let label = places[at].label.clone();
        if building.is_empty() && quiet_minutes.is_none_or(|m| m >= QUIET_MINUTES) {
            findings.push(format!(
                "worktree: {label} ({}) has had no change for {} and no build runs there",
                claim.who,
                quiet_minutes.map(|m| format!("{m}m")).unwrap_or_else(|| "as long as git can tell".to_string())
            ));
        }
        worktrees.push(WorktreeHealth {
            checkout: label,
            path: root.top.display().to_string(),
            claim,
            branch: checkout.branch()?,
            git,
            quiet_minutes,
            building,
        });
    }

    // App locks.
    let mut locks = Vec::new();
    let mut repos: Vec<String> = roots.iter().map(root_repo).collect();
    repos.dedup();
    for repo in repos {
        let path = tool.places.root.join(&repo).join("app.lock");
        if !path.is_file() {
            continue;
        }
        let shown = path.display().to_string();
        let real = std::fs::canonicalize(&path).unwrap_or(path.clone());
        let mut holder = None;
        let mut waiting = 0;
        for process in processes.list.iter().filter(|p| p.name == "lockf") {
            if !args(process.pid).is_some_and(|argv| argv.contains(&shown)) {
                continue;
            }
            if open_files(process.pid).iter().any(|open| *open == real || *open == path) {
                holder = Some(Holder {
                    pid: process.pid,
                    minutes: minutes_since(now, process.started),
                    command: command_line(process.pid),
                    running: processes.children(process.pid).map(|child| command_line(child.pid)).collect(),
                });
            } else {
                waiting += 1;
            }
        }
        locks.push(LockHealth { repo, path: shown, holder, waiting });
    }

    Ok(Health {
        generated_at: now,
        load: load_average(),
        cores: cores(),
        free_kib,
        floor_kib: tool.rules.floor_kib,
        hard_floor_kib: tool.rules.hard_floor_kib,
        sccache: SccacheHealth { listening, executed, compared, waiting },
        shells,
        worktrees,
        locks,
        findings,
    })
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
            w.claim.note.as_ref().map(|n| format!(": {n}")).unwrap_or_default()
        ));
        out.push(format!(
            "        {git}; last change {}; {}",
            w.quiet_minutes.map(|m| format!("{m}m ago")).unwrap_or_else(|| "unknown".to_string()),
            if w.building.is_empty() { "no build running".to_string() } else { format!("building: {}", process_summary(&w.building)) }
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
    let health = health(tool)?;
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
        assert!(runs_the_dashboard("~/dev/agent-instructions/scripts/builds/builds dashboard --port 7787"));
        assert!(!runs_the_dashboard("builds status"));
    }
}
