//! Processes that have used a lot of CPU for a long time, on this Mac and on
//! every reachable host: what `builds health` lists so a process stuck at
//! full speed (a build past its time limit, a test binary that never ends, a
//! system daemon such as fseventsd pegged for days) does not run unseen.
//!
//! Both Macs are read the same way: one `ps` listing (run here, or over the
//! ssh builds already uses for hosts), judged by `judge`, which only reads
//! text. The working folders that place a process in a checkout come from
//! libproc here and from `lsof` on a host, only for the busy processes and
//! their ancestors. Nothing here stops a process: a finding says what to run.

use std::collections::HashMap;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::app_config::DEFAULT_STEP_LIMIT;
use crate::host::{Host, Ssh};
use crate::shells::{basename, command_of, is_agent_argv, shown};

/// Busy now: over this share of a core and over BUSY_CPU_SECONDS of CPU time.
const BUSY_PERCENT: f64 = 50.0;
const BUSY_CPU_SECONDS: f64 = 10.0 * 60.0;
/// Or at this share or more, for this long.
const PEGGED_PERCENT: f64 = 90.0;
const PEGGED_SECONDS: i64 = 30 * 60;
/// A test binary running this long has run longer than a test run takes.
const TEST_SECONDS: i64 = 30 * 60;
/// A system daemon with this much CPU time, still busy, is pegged.
const DAEMON_CPU_SECONDS: f64 = 2.0 * 60.0 * 60.0;
/// How many ancestors of a process are read.
const ANCESTORS: usize = 12;

/// What `ps` prints, one process a line: pid, parent, user, CPU share, CPU
/// time, time since start, command with its arguments.
const PS_FORMAT: &str = "pid=,ppid=,uid=,pcpu=,cputime=,etime=,command=";

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    /// The share of a core in use now, as a percentage.
    pub cpu_percent: f64,
    pub cpu_seconds: f64,
    pub age_seconds: i64,
    pub command: String,
}

/// `[[dd-]hh:]mm:ss` as seconds.
fn parse_elapsed(text: &str) -> Option<i64> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<i64>().ok()?, clock),
        None => (0, text),
    };
    let mut seconds = 0;
    for part in clock.split(':') {
        seconds = seconds * 60 + part.parse::<i64>().ok()?;
    }
    Some(days * 86_400 + seconds)
}

/// `[hh:]mm:ss.cc` as seconds; minutes may pass 59.
fn parse_cpu_time(text: &str) -> Option<f64> {
    let mut seconds = 0.0;
    for part in text.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(seconds)
}

/// The rows of <text>, `ps`'s output for `PS_ARGS`. A line that does not read
/// is an error naming it.
pub fn parse_ps(text: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut words = line.split_whitespace();
        let mut next = || words.next().ok_or_else(|| format!("short ps line: {line}"));
        let bad = |what: &str| format!("ps line with a bad {what}: {line}");
        let pid = next()?.parse().map_err(|_| bad("pid"))?;
        let ppid = next()?.parse().map_err(|_| bad("parent"))?;
        let uid = next()?.parse().map_err(|_| bad("user"))?;
        let cpu_percent = next()?.parse().map_err(|_| bad("CPU share"))?;
        let cpu_seconds = parse_cpu_time(next()?).ok_or_else(|| bad("CPU time"))?;
        let age_seconds = parse_elapsed(next()?).ok_or_else(|| bad("age"))?;
        // The command is the rest of the line, spaces kept.
        let command = {
            let mut rest = line.trim_start();
            for _ in 0..6 {
                rest = rest
                    .trim_start_matches(|c: char| !c.is_whitespace())
                    .trim_start();
            }
            rest.to_string()
        };
        rows.push(Row {
            pid,
            ppid,
            uid,
            cpu_percent,
            cpu_seconds,
            age_seconds,
            command,
        });
    }
    Ok(rows)
}

/// What a busy process belongs to.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Owner {
    /// A process of a build `builds` started: `builds app-build bae <branch>
    /// macos-tests` and the compilers under it.
    Builds {
        pid: i32,
        /// The builds command, from its subcommand on.
        command: String,
        /// The checkout (`repo/side-N`) it works in, when one can be told.
        checkout: Option<String>,
        /// Minutes the builds process has run.
        minutes: i64,
    },
    /// A process under an agent's shell.
    Agent {
        shell: i32,
        checkout: Option<String>,
    },
    /// A test binary cargo built.
    Test {
        checkout: Option<String>,
    },
    /// A process of the system, run as root or from a system folder.
    System {
        name: String,
    },
    Other,
}

impl Owner {
    fn describe(&self) -> String {
        match self {
            Owner::Builds {
                pid,
                command,
                checkout,
                minutes,
            } => format!(
                "build started by builds pid {pid} ({}m): builds {}{}",
                minutes,
                shown(command),
                checkout
                    .as_ref()
                    .map(|c| format!(", in {c}"))
                    .unwrap_or_default()
            ),
            Owner::Agent { shell, checkout } => format!(
                "under agent shell pid {shell}{}",
                checkout
                    .as_ref()
                    .map(|c| format!(" in {c}"))
                    .unwrap_or_default()
            ),
            Owner::Test { checkout } => format!(
                "test binary{}",
                checkout
                    .as_ref()
                    .map(|c| format!(" in {c}"))
                    .unwrap_or_default()
            ),
            Owner::System { name } => format!("system daemon {name}"),
            Owner::Other => "no builds or agent parent".to_string(),
        }
    }
}

/// One process that has used a lot of CPU for a long time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Busy {
    /// The host's name; `local` for this Mac.
    pub host: String,
    pub pid: i32,
    pub cpu_percent: f64,
    pub cpu_seconds: f64,
    pub age_seconds: i64,
    pub command: String,
    pub owner: Owner,
    /// What the person does about it, when the process looks stuck.
    pub action: Option<String>,
}

fn is_busy(row: &Row) -> bool {
    (row.cpu_percent > BUSY_PERCENT && row.cpu_seconds > BUSY_CPU_SECONDS)
        || (row.cpu_percent >= PEGGED_PERCENT && row.age_seconds >= PEGGED_SECONDS)
}

/// The pids whose working folders `judge` reads: every busy process and its
/// ancestors.
pub fn pids_to_place(rows: &[Row]) -> Vec<i32> {
    let by_pid: HashMap<i32, &Row> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut pids = Vec::new();
    for row in rows.iter().filter(|r| is_busy(r)) {
        for at in chain(&by_pid, row) {
            if !pids.contains(&at.pid) {
                pids.push(at.pid);
            }
        }
    }
    pids
}

/// <row> and its ancestors, nearest first.
fn chain<'a>(by_pid: &HashMap<i32, &'a Row>, row: &'a Row) -> Vec<&'a Row> {
    let mut chain = vec![row];
    while chain.len() < ANCESTORS {
        let Some(parent) = by_pid.get(&chain[chain.len() - 1].ppid) else {
            break;
        };
        if chain.iter().any(|r| r.pid == parent.pid) {
            break;
        }
        chain.push(parent);
    }
    chain
}

/// The checkout a path is in: `repo/side-N` for `~/dev/<repo>-worktrees/side-N`
/// and `/Volumes/Builds/<repo>/<checkout>`, `repo/main` for `~/dev/<repo>`.
pub fn checkout_label(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let worktree = parts.iter().enumerate().find_map(|(at, part)| {
        let repo = part.strip_suffix("-worktrees")?;
        Some(format!("{repo}/{}", parts.get(at + 1)?))
    });
    let volume = parts.windows(4).find_map(|w| {
        (w[0] == "Volumes" && w[1] == "Builds").then(|| format!("{}/{}", w[2], w[3]))
    });
    let main = parts
        .windows(3)
        .find_map(|w| (w[0] == "dev" && !w[1].contains('.')).then(|| format!("{}/main", w[1])));
    worktree.or(volume).or(main)
}

/// The checkout of the first of <rows> that names one, in its working folder
/// or in the words of its command.
fn checkout_of(rows: &[&Row], cwds: &HashMap<i32, String>) -> Option<String> {
    rows.iter().find_map(|row| {
        cwds.get(&row.pid)
            .and_then(|dir| checkout_label(dir))
            .or_else(|| row.command.split_whitespace().find_map(checkout_label))
    })
}

fn is_system(row: &Row) -> bool {
    let program = row.command.split_whitespace().next().unwrap_or("");
    row.uid == 0
        || ["/System/", "/usr/libexec/", "/usr/sbin/", "/sbin/"]
            .iter()
            .any(|prefix| program.starts_with(prefix))
}

/// `name-<16 hex digits>`, the file name of a test binary cargo builds.
fn is_test_binary(row: &Row) -> bool {
    let program = row.command.split_whitespace().next().unwrap_or("");
    basename(program)
        .rsplit_once('-')
        .is_some_and(|(name, hash)| {
            !name.is_empty() && hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit())
        })
        && program.contains("/target/")
}

fn owner_of(by_pid: &HashMap<i32, &Row>, row: &Row, cwds: &HashMap<i32, String>) -> Owner {
    let ancestors = chain(by_pid, row);
    if is_test_binary(row) {
        return Owner::Test {
            checkout: checkout_of(&ancestors, cwds),
        };
    }
    for (at, ancestor) in ancestors.iter().enumerate() {
        let words: Vec<&str> = ancestor.command.split_whitespace().collect();
        if words.first().is_some_and(|w| basename(w) == "builds") {
            return Owner::Builds {
                pid: ancestor.pid,
                command: words[1..].join(" "),
                checkout: checkout_of(&ancestors[at..], cwds)
                    .or_else(|| checkout_of(&ancestors, cwds)),
                minutes: ancestor.age_seconds / 60,
            };
        }
        let argv: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        if is_agent_argv(&argv) {
            return Owner::Agent {
                shell: ancestor.pid,
                checkout: checkout_of(&ancestors[at..], cwds)
                    .or_else(|| checkout_of(&ancestors, cwds)),
            };
        }
    }
    if is_system(row) {
        let program = row.command.split_whitespace().next().unwrap_or("");
        return Owner::System {
            name: basename(program).to_string(),
        };
    }
    Owner::Other
}

fn minutes(seconds: i64) -> i64 {
    seconds / 60
}

/// What the person does about <busy>, when it looks stuck.
fn action_for(owner: &Owner, row: &Row) -> Option<String> {
    let limit = DEFAULT_STEP_LIMIT.as_secs() as i64;
    let stop = format!("stop it with `kill {}`", row.pid);
    match owner {
        Owner::Builds { minutes: m, .. } if m * 60 >= limit => Some(format!(
            "its build has run {m}m, past the {}m step limit; {stop}",
            minutes(limit)
        )),
        Owner::Agent { .. } if row.age_seconds >= limit => Some(format!(
            "it has run {}m under an agent, past the {}m step limit; {stop}",
            minutes(row.age_seconds),
            minutes(limit)
        )),
        Owner::Test { .. } if row.age_seconds >= TEST_SECONDS => Some(format!(
            "this test binary has run {}m, longer than a test run takes ({}m); {stop}",
            minutes(row.age_seconds),
            minutes(TEST_SECONDS)
        )),
        Owner::System { name } if row.cpu_seconds >= DAEMON_CPU_SECONDS => Some(format!(
            "this system daemon has used {} of CPU and still runs busy; run `sudo killall {name}`",
            duration(row.cpu_seconds as i64)
        )),
        _ => None,
    }
}

/// <seconds> as `6d2h`, `3h05m` or `12m`.
fn duration(seconds: i64) -> String {
    let (days, hours, mins) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins:02}m")
    } else {
        format!("{mins}m")
    }
}

/// The busy processes among <rows> on <host>, most CPU time first, each with
/// its owner. <cwds> are the working folders of `pids_to_place(rows)`.
pub fn judge(host: &str, rows: &[Row], cwds: &HashMap<i32, String>) -> Vec<Busy> {
    let by_pid: HashMap<i32, &Row> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut found: Vec<Busy> = rows
        .iter()
        .filter(|row| is_busy(row))
        .map(|row| {
            let owner = owner_of(&by_pid, row, cwds);
            let action = action_for(&owner, row);
            Busy {
                host: host.to_string(),
                pid: row.pid,
                cpu_percent: row.cpu_percent,
                cpu_seconds: row.cpu_seconds,
                age_seconds: row.age_seconds,
                command: row.command.clone(),
                owner,
                action,
            }
        })
        .collect();
    found.sort_by(|a, b| b.cpu_seconds.total_cmp(&a.cpu_seconds));
    found
}

impl Busy {
    pub fn line(&self) -> String {
        format!(
            "    {} pid {}: {:.0}% now, {} CPU, {} old; {}; {}",
            self.host,
            self.pid,
            self.cpu_percent,
            duration(self.cpu_seconds as i64),
            duration(self.age_seconds),
            self.owner.describe(),
            shown(&command_of(
                &self
                    .command
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            ))
        )
    }

    /// The line under "needs action", when the process looks stuck.
    pub fn finding(&self) -> Option<String> {
        self.action.as_ref().map(|action| {
            format!(
                "busy: {} pid {} ({}) {action}",
                self.host,
                self.pid,
                shown(basename(
                    self.command.split_whitespace().next().unwrap_or("")
                ))
            )
        })
    }
}

/// What reading one host gave.
pub struct HostBusy {
    pub host: String,
    pub busy: Result<Vec<Busy>, String>,
}

fn ps_command() -> Command {
    let mut command = Command::new("ps");
    command.args(["-axww", "-o", PS_FORMAT]);
    command
}

/// The `ps` line of a host, as the script `ssh` runs.
fn ps_script() -> String {
    format!("ps -axww -o {PS_FORMAT}")
}

/// The folders `lsof -Fpn` lists, as pid to path.
pub fn parse_lsof(text: &str) -> HashMap<i32, String> {
    let mut pid = None;
    let mut found = HashMap::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            pid = rest.parse().ok();
        } else if let (Some(path), Some(pid)) = (line.strip_prefix('n'), pid) {
            found.insert(pid, path.to_string());
        }
    }
    found
}

fn output_of(mut command: Command, what: &str) -> Result<String, String> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("running {what}: {error}"))?;
    if !output.status.success() && output.stdout.is_empty() {
        return Err(format!(
            "{what} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Reads <host>'s processes and judges them.
pub fn read(ssh: &Ssh, host: &Host) -> HostBusy {
    let busy = (|| {
        let listing = match &host.target {
            Some(target) => output_of(
                ssh.command(target, &ps_script()),
                &format!("ps on host {}", host.name),
            )?,
            None => output_of(ps_command(), "ps")?,
        };
        let rows = parse_ps(&listing)?;
        let pids = pids_to_place(&rows);
        let cwds = if pids.is_empty() {
            HashMap::new()
        } else {
            match &host.target {
                Some(target) => {
                    let list: Vec<String> = pids.iter().map(|p| p.to_string()).collect();
                    parse_lsof(&output_of(
                        ssh.command(
                            target,
                            &format!("lsof -a -d cwd -Fpn -p {}", list.join(",")),
                        ),
                        &format!("lsof on host {}", host.name),
                    )?)
                }
                None => pids
                    .iter()
                    .filter_map(|pid| {
                        crate::procs::cwd(*pid).map(|dir| (*pid, dir.display().to_string()))
                    })
                    .collect(),
            }
        };
        Ok(judge(&host.name, &rows, &cwds))
    })();
    HostBusy {
        host: host.name.clone(),
        busy,
    }
}

/// The lines `builds health` prints for <hosts>.
pub fn lines(hosts: &[HostBusy]) -> Vec<String> {
    let mut out = vec![format!(
        "busy processes (over {BUSY_PERCENT:.0}% CPU now and {}m CPU, or {PEGGED_PERCENT:.0}%+ for {}m):",
        minutes(BUSY_CPU_SECONDS as i64),
        minutes(PEGGED_SECONDS)
    )];
    for host in hosts {
        match &host.busy {
            Err(error) => out.push(format!("    {}: not read: {error}", host.host)),
            Ok(busy) if busy.is_empty() => out.push(format!("    {}: none", host.host)),
            Ok(busy) => out.extend(busy.iter().map(Busy::line)),
        }
    }
    out
}

/// The "needs action" lines for <hosts>.
pub fn findings(hosts: &[HostBusy]) -> Vec<String> {
    hosts
        .iter()
        .filter_map(|host| host.busy.as_ref().ok())
        .flatten()
        .filter_map(Busy::finding)
        .collect()
}

#[cfg(test)]
#[path = "stuck_tests.rs"]
mod tests;
