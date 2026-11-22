//! Processes that have used a lot of CPU for a long time, on this Mac and on
//! every reachable host: what `builds health` lists so a process stuck at
//! full speed (a build past its time limit, a test binary that never ends, a
//! system daemon such as fseventsd pegged for days) does not run unseen.
//!
//! Also the test binaries `builds cargo` runs that sit idle (`Hung`): a test
//! that hangs uses no CPU, and the `builds cargo` waiting on it, here or over
//! ssh from this Mac, uses none either, so neither the busy rule nor the
//! agent shells' idle rule (which spares a shell running a build) sees it.
//!
//! Both Macs are read the same way: the `ps` listing of a host's scan
//! (host_scan.rs, run here or over the ssh builds already uses for hosts),
//! judged by `judge`, which only reads text. The working folders that place
//! a process in a checkout come from libproc here and from `lsof` on a host,
//! only for the busy processes and their ancestors. Nothing here stops a
//! process: a finding says what to run.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::app_config::DEFAULT_STEP_LIMIT;
use crate::health::Advice;
use crate::host::{Host, Ssh, command_at, command_output, output_of};
use crate::shells::{IDLE_MINUTES, IDLE_SHARE, basename, command_of, is_agent_argv, shown};
use crate::text::duration;

/// Busy now: over this share of a core, over the host scan's window, and over
/// BUSY_CPU_SECONDS of CPU time since the process started.
const BUSY_PERCENT: f64 = 50.0;
const BUSY_CPU_SECONDS: f64 = 10.0 * 60.0;
/// Or at this share or more, for this long.
const PEGGED_PERCENT: f64 = 90.0;
const PEGGED_SECONDS: i64 = 30 * 60;
/// A test binary running this long has run longer than a test run takes.
const TEST_SECONDS: i64 = 30 * 60;
/// fseventsd is flagged as soon as it has kept busy for a span, like every
/// system daemon: every program that watches files depends on it, and a cause
/// is only found by sampling it while it runs.
const FSEVENTSD: &str = "fseventsd";
/// Seconds `sample` watches the process.
const SAMPLE_SECONDS: u32 = 5;
/// How many ancestors of a process are read.
const ANCESTORS: usize = 12;

/// What `ps` prints, one process a line: pid, parent, user, CPU share, CPU
/// time, time since start, state, command with its arguments.
pub const PS_FORMAT: &str = "pid=,ppid=,uid=,pcpu=,cputime=,etime=,stat=,command=";

/// What a process used over a span of the host's readings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Sustained {
    /// The lowest share of a core, as a percentage, in any interval of the span.
    pub percent: f64,
    /// The seconds the span covers.
    pub seconds: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    /// The share of a core the process used since the host's previous
    /// reading (process_readings.rs), as a percentage, or None when that
    /// reading has no such process. Rows from `parse_ps` hold what `ps`
    /// prints, and a scan replaces it with the measured share.
    pub cpu_percent: Option<f64>,
    /// The lowest share the process had in any interval between the host's
    /// readings over the last span (process_readings.rs), or None when the
    /// readings do not cover a span with the process in every interval.
    pub sustained: Option<Sustained>,
    pub cpu_seconds: f64,
    pub age_seconds: i64,
    /// Exited and waiting for its parent to reap it (state Z).
    pub zombie: bool,
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

/// `[hh:]mm:ss.cc` (macOS) or `[dd-]hh:mm:ss` (Linux) as seconds; minutes may
/// pass 59.
fn parse_cpu_time(text: &str) -> Option<f64> {
    let (days, text) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<f64>().ok()?, clock),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in text.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + seconds)
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
        let zombie = next()?.starts_with('Z');
        // The command is the rest of the line, spaces kept.
        let command = {
            let mut rest = line.trim_start();
            for _ in 0..7 {
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
            cpu_percent: Some(cpu_percent),
            sustained: None,
            cpu_seconds,
            age_seconds,
            zombie,
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
        /// What it used over the span that made it busy.
        span: Sustained,
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
            Owner::System { name, .. } => format!("system daemon {name}"),
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
    pub action: Option<Advice>,
}

/// The CPU share of <row> when it is busy, and None when it is not or its
/// share is not known yet.
fn busy_share(row: &Row) -> Option<f64> {
    let percent = row.cpu_percent?;
    ((percent > BUSY_PERCENT && row.cpu_seconds > BUSY_CPU_SECONDS)
        || (percent >= PEGGED_PERCENT && row.age_seconds >= PEGGED_SECONDS))
        .then_some(percent)
}

fn is_busy(row: &Row) -> bool {
    busy_share(row).is_some()
}

/// The pids whose working folders `judge` and `hung_tests` read: every busy
/// process and every hung test, with their ancestors.
pub fn pids_to_place(rows: &[Row]) -> Vec<i32> {
    let by_pid: HashMap<i32, &Row> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut pids = Vec::new();
    for row in rows
        .iter()
        .filter(|r| is_busy(r) || is_hung_test(&by_pid, r))
    {
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
pub fn is_test_binary(row: &Row) -> bool {
    let program = row.command.split_whitespace().next().unwrap_or("");
    basename(program)
        .rsplit_once('-')
        .is_some_and(|(name, hash)| {
            !name.is_empty() && hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit())
        })
        && program.contains("/target/")
}

/// Is <row> a test binary that a `builds cargo` above it runs, that has run
/// IDLE_MINUTES or more and used less than IDLE_SHARE of a core since it
/// started? This is the rule `builds health` uses to call an agent shell idle
/// (shells.rs), because a test that works uses CPU.
fn is_hung_test(by_pid: &HashMap<i32, &Row>, row: &Row) -> bool {
    let age = row.age_seconds as f64;
    is_test_binary(row)
        && row.age_seconds >= IDLE_MINUTES * 60
        && row.cpu_seconds < IDLE_SHARE * age
        && chain(by_pid, row).iter().any(|ancestor| {
            let mut words = ancestor.command.split_whitespace();
            words.next().is_some_and(|w| basename(w) == "builds") && words.next() == Some("cargo")
        })
}

/// A test binary that `builds cargo` runs and that sits idle.
#[derive(Clone, Debug, PartialEq)]
pub struct Hung {
    /// The host's name; `local` for this Mac.
    pub host: String,
    pub pid: i32,
    pub cpu_seconds: f64,
    pub age_seconds: i64,
    pub command: String,
    /// The worktree (`repo/side-N`, or `repo/remote-N` on a host) it runs
    /// in, when `checkout_of` finds one.
    pub checkout: Option<String>,
    /// The command that stops it.
    pub stop: String,
}

impl Hung {
    /// The line under "needs action".
    pub fn finding(&self) -> Advice {
        Advice::run(
            format!(
                "hung test: {} pid {} in {} has sat idle for {}: it used {:.1}% of a core since it started, under the {:.0}% that marks an agent shell idle after {IDLE_MINUTES}m, and the builds cargo that runs it waits on it. The test's command is {}. Stop it with",
                self.host,
                self.pid,
                self.checkout.as_deref().unwrap_or("an unknown worktree"),
                duration(self.age_seconds),
                self.cpu_seconds / self.age_seconds.max(1) as f64 * 100.0,
                IDLE_SHARE * 100.0,
                shown(&self.command),
            ),
            self.stop.clone(),
        )
    }
}

/// The hung tests among <rows> on <host>. <cwds> are the working folders of
/// `pids_to_place(rows)`.
pub fn hung_tests(
    host: &str,
    rows: &[Row],
    cwds: &HashMap<i32, String>,
    sampling: &Sampling,
) -> Vec<Hung> {
    let by_pid: HashMap<i32, &Row> = rows.iter().map(|r| (r.pid, r)).collect();
    rows.iter()
        .filter(|row| is_hung_test(&by_pid, row))
        .map(|row| Hung {
            host: host.to_string(),
            pid: row.pid,
            cpu_seconds: row.cpu_seconds,
            age_seconds: row.age_seconds,
            command: row.command.clone(),
            checkout: checkout_of(&chain(&by_pid, row), cwds),
            stop: sampling.on_host(&format!("kill {}", row.pid)),
        })
        .collect()
}

/// What <row> belongs to, or None when it is a system daemon that has not
/// stayed over BUSY_PERCENT in every interval of a span: it is busy only
/// while it keeps using CPU, not for one interval.
fn owner_of(by_pid: &HashMap<i32, &Row>, row: &Row, cwds: &HashMap<i32, String>) -> Option<Owner> {
    let ancestors = chain(by_pid, row);
    if is_test_binary(row) {
        return Some(Owner::Test {
            checkout: checkout_of(&ancestors, cwds),
        });
    }
    for (at, ancestor) in ancestors.iter().enumerate() {
        let words: Vec<&str> = ancestor.command.split_whitespace().collect();
        if words.first().is_some_and(|w| basename(w) == "builds") {
            return Some(Owner::Builds {
                pid: ancestor.pid,
                command: words[1..].join(" "),
                checkout: checkout_of(&ancestors[at..], cwds)
                    .or_else(|| checkout_of(&ancestors, cwds)),
                minutes: ancestor.age_seconds / 60,
            });
        }
        let argv: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        if is_agent_argv(&argv) {
            return Some(Owner::Agent {
                shell: ancestor.pid,
                checkout: checkout_of(&ancestors[at..], cwds)
                    .or_else(|| checkout_of(&ancestors, cwds)),
            });
        }
    }
    if is_system(row) {
        let program = row.command.split_whitespace().next().unwrap_or("");
        let span = row.sustained.filter(|s| s.percent > BUSY_PERCENT)?;
        return Some(Owner::System {
            name: basename(program).to_string(),
            span,
        });
    }
    Some(Owner::Other)
}

fn minutes(seconds: i64) -> i64 {
    seconds / 60
}

/// Where a host's samples go and when the command is written, so a file name
/// holds the host, the pid and the time.
#[derive(Clone, Debug, PartialEq)]
pub struct Sampling {
    /// The home folder as the host's shell writes it: the real path here,
    /// `$HOME` over ssh.
    home: String,
    /// The ssh target, when the host is not this Mac.
    target: Option<String>,
    /// Unix seconds.
    at: i64,
}

impl Sampling {
    pub fn new(host: &Host, at: i64) -> Sampling {
        Sampling {
            home: match host.target {
                Some(_) => "$HOME".to_string(),
                None => std::env::var("HOME").unwrap_or_else(|_| "$HOME".to_string()),
            },
            target: host.target.clone(),
            at,
        }
    }

    /// The command the person runs to sample <pid> into builds' store. It
    /// makes the folder and runs `sudo sample`; health never runs sudo.
    fn command(&self, host: &str, pid: i32) -> String {
        let dir = format!("{}/Library/Application Support/builds/samples", self.home);
        let file = format!("{dir}/{host}-{pid}-{}.txt", self.at);
        let script =
            format!("mkdir -p \"{dir}\" && sudo sample {pid} {SAMPLE_SECONDS} -file \"{file}\"");
        match &self.target {
            Some(target) => format!("ssh -t {target} '{script}'"),
            None => script,
        }
    }

    /// <script> as the person runs it on the host: over ssh, or as it is
    /// on this Mac.
    pub fn on_host(&self, script: &str) -> String {
        crate::host::person_command(self.target.as_deref(), script)
    }
}

fn is_fseventsd(row: &Row) -> bool {
    is_system(row) && basename(row.command.split_whitespace().next().unwrap_or("")) == FSEVENTSD
}

/// What the person does about <busy>, when it looks stuck.
fn action_for(host: &str, owner: &Owner, row: &Row, sampling: &Sampling) -> Option<Advice> {
    let limit = DEFAULT_STEP_LIMIT.as_secs() as i64;
    let kill = format!("kill {}", row.pid);
    let sample = sampling.command(host, row.pid);
    match owner {
        Owner::Builds { minutes: m, .. } if m * 60 >= limit => Some(Advice::run(
            format!(
                "its build has run {m}m, past the {}m step limit; stop it with",
                minutes(limit)
            ),
            kill,
        )),
        Owner::Agent { .. } if row.age_seconds >= limit => Some(Advice::run(
            format!(
                "it has run {}m under an agent, past the {}m step limit; stop it with",
                minutes(row.age_seconds),
                minutes(limit)
            ),
            kill,
        )),
        Owner::Test { .. } if row.age_seconds >= TEST_SECONDS => Some(Advice::run(
            format!(
                "this test binary has run {}m, longer than a test run takes ({}m); stop it with",
                minutes(row.age_seconds),
                minutes(TEST_SECONDS)
            ),
            kill,
        )),
        Owner::System { name, span } if name == FSEVENTSD => Some(Advice::run(
            format!(
                "fseventsd used {} of CPU and stayed over {BUSY_PERCENT:.0}% of a core in every interval of the last {}m; read a sample of it before anyone kills fseventsd, and take the sample with",
                duration(row.cpu_seconds as i64),
                minutes(span.seconds),
            ),
            sample,
        )),
        Owner::System { span, .. } => Some(Advice::run(
            format!(
                "this system daemon stayed over {BUSY_PERCENT:.0}% of a core in every interval of the last {}m (at least {:.0}%); read a sample of it before anyone kills it, and take the sample with",
                minutes(span.seconds),
                span.percent,
            ),
            sample,
        )),
        _ => None,
    }
}

/// The busy processes among <rows> on <host>, most CPU time first, each with
/// its owner. <cwds> are the working folders of `pids_to_place(rows)`. The
/// dashboard (<dashboard>, its pid on this host) is never one: it runs as
/// long as the machine is up and is busy whenever the volume changes a lot,
/// and health checks it only for being up.
pub fn judge(
    host: &str,
    rows: &[Row],
    cwds: &HashMap<i32, String>,
    sampling: &Sampling,
    dashboard: Option<i32>,
) -> Vec<Busy> {
    let by_pid: HashMap<i32, &Row> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut found: Vec<Busy> = rows
        .iter()
        .filter(|row| Some(row.pid) != dashboard)
        .filter_map(|row| Some((row, busy_share(row)?)))
        .filter_map(|(row, percent)| {
            let owner = owner_of(&by_pid, row, cwds)?;
            let action = action_for(host, &owner, row, sampling);
            Some(Busy {
                host: host.to_string(),
                pid: row.pid,
                cpu_percent: percent,
                cpu_seconds: row.cpu_seconds,
                age_seconds: row.age_seconds,
                command: row.command.clone(),
                owner,
                action,
            })
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
    pub fn finding(&self) -> Option<Advice> {
        self.action.as_ref().map(|action| Advice {
            text: format!(
                "busy: {} pid {} ({}) {}",
                self.host,
                self.pid,
                shown(basename(
                    self.command.split_whitespace().next().unwrap_or("")
                )),
                action.text
            ),
            command: action.command.clone(),
        })
    }
}

/// An fseventsd that refuses every client.
#[derive(Clone, Debug, PartialEq)]
pub struct Wedged {
    pub pid: i32,
    pub finding: Advice,
}

/// <event_id> is what `FSEventsGetCurrentEventId` returned on <host>: 0 means
/// fseventsd does not answer. A running fseventsd that does not answer is
/// wedged, busy or not, and a plain `kill` does not end it.
pub fn wedged(host: &str, rows: &[Row], event_id: u64, sampling: &Sampling) -> Option<Wedged> {
    if event_id != 0 {
        return None;
    }
    let row = rows.iter().find(|row| is_fseventsd(row))?;
    let sample = sampling.command(host, row.pid);
    Some(Wedged {
        pid: row.pid,
        finding: Advice::run(
            format!(
                "fseventsd does not answer: {host} pid {pid} has run {} and refuses every client (its current event id is 0). A plain kill does not end an fseventsd that does not answer, so end it with sudo kill -9 {pid} once you have read a sample of it. Take the sample with",
                duration(row.age_seconds),
                pid = row.pid,
            ),
            sample,
        ),
    })
}

/// What reading one host gave: what its processes showed, or why `read`
/// could not list them.
pub struct HostBusy {
    pub host: String,
    pub read: Result<HostRead, String>,
}

/// What one listing of a host's processes showed.
pub struct HostRead {
    pub busy: Vec<Busy>,
    /// Why lsof could only place some of the processes.
    pub placement_diagnostic: Option<String>,
    /// Set on this Mac only, the one host whose fseventsd health can ask.
    pub wedged: Option<Wedged>,
    pub hung: Vec<Hung>,
    /// The parents that hold the most zombie children (proc_limit.rs).
    pub zombie_parents: Vec<crate::proc_limit::ZombieParent>,
}

impl HostBusy {
    /// Why either the entire read or only some process placements failed.
    pub fn error(&self) -> Option<&str> {
        match &self.read {
            Ok(read) => read.placement_diagnostic.as_deref(),
            Err(error) => Some(error),
        }
    }
}

/// The rows of <host>'s processes, from one `ps` there.
pub fn rows_on(ssh: &Ssh, host: &Host) -> Result<Vec<Row>, String> {
    let listing = output_of(
        command_at(ssh, host, &ps_script(), crate::host::Terminal::None),
        &format!("ps on {}", crate::host::place(&host.name)),
    )?;
    parse_ps(&listing)
}

/// The `ps` line of a host, as the script `ssh` runs.
fn ps_script() -> String {
    format!("ps -axww -o {PS_FORMAT}")
}

/// The start time of each process that still runs, by the host's clock.
fn live_starts(ssh: &Ssh, host: &Host) -> Result<HashMap<i32, i64>, String> {
    let what = format!("process lifetimes on {}", crate::host::place(&host.name));
    let script = format!("date +%s && {}", ps_script());
    let listing = output_of(
        command_at(ssh, host, &script, crate::host::Terminal::None),
        &what,
    )?;
    let (at, rows) = listing
        .split_once('\n')
        .ok_or_else(|| format!("{what}: ps printed no process listing"))?;
    let at = at
        .parse::<i64>()
        .map_err(|_| format!("{what}: date printed an invalid timestamp {at:?}"))?;
    Ok(parse_ps(rows)?
        .into_iter()
        .filter(|row| !row.zombie)
        .map(|row| (row.pid, at - row.age_seconds))
        .collect())
}

/// The working folders found by lsof and any diagnostic beside those rows.
fn folders_on(
    ssh: &Ssh,
    host: &Host,
    pids: &[i32],
) -> Result<(HashMap<i32, String>, Option<String>), String> {
    let list: Vec<String> = pids.iter().map(|pid| pid.to_string()).collect();
    let what = format!("lsof on {}", crate::host::place(&host.name));
    let output = command_output(
        command_at(
            ssh,
            host,
            &format!("lsof -a -d cwd -Fpn -p {}", list.join(",")),
            crate::host::Terminal::None,
        ),
        &what,
    )?;
    let folders = parse_lsof(&String::from_utf8_lossy(&output.stdout));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    let diagnostic = (!stderr.is_empty()).then(|| format!("{what} wrote: {stderr}"));
    if output.status.success()
        || (output.status.code() == Some(1) && (diagnostic.is_none() || !folders.is_empty()))
    {
        return Ok((folders, diagnostic));
    }
    Err(format!(
        "{what} failed ({}){}",
        output.status,
        diagnostic.map_or_else(String::new, |said| format!(". {said}"))
    ))
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

pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs() as i64
}

/// Judges the processes that <scan> read on <host>, and skips the dashboard,
/// whose pid on the host is <dashboard>.
pub fn read(
    ssh: &Ssh,
    host: &Host,
    scan: &Result<crate::host_scan::Scan, String>,
    dashboard: Option<i32>,
) -> HostBusy {
    let read = (|| {
        let scan = scan.as_ref().map_err(String::clone)?;
        let mut rows = Cow::Borrowed(scan.rows.as_slice());
        let pids = pids_to_place(&rows);
        let mut placement_diagnostic = None;
        let cwds = if pids.is_empty() {
            HashMap::new()
        } else {
            match &host.target {
                Some(_) => {
                    let (cwds, diagnostic) = folders_on(ssh, host, &pids)?;
                    placement_diagnostic = diagnostic;
                    let unplaced: HashSet<_> = pids
                        .iter()
                        .copied()
                        .filter(|pid| !cwds.contains_key(pid))
                        .collect();
                    if !unplaced.is_empty() {
                        // lsof can omit a living process whose working folder
                        // it cannot see. Recheck only the unplaced rows' lifetimes.
                        let live = live_starts(ssh, host)?;
                        rows.to_mut().retain(|row| {
                            if !unplaced.contains(&row.pid)
                                || live.get(&row.pid).is_some_and(|start| {
                                    crate::process_readings::same_start(
                                        scan.at - row.age_seconds,
                                        *start,
                                    )
                                })
                            {
                                return true;
                            }
                            crate::text::note(format!(
                                "busy processes on {}: PID {} exited after the scan",
                                host.name, row.pid
                            ));
                            false
                        });
                    }
                    cwds
                }
                None => pids
                    .iter()
                    .filter_map(|pid| {
                        crate::procs::cwd(*pid).map(|dir| (*pid, dir.display().to_string()))
                    })
                    .collect(),
            }
        };
        let sampling = Sampling::new(host, now_seconds());
        let wedged = match host.target {
            None => wedged(
                &host.name,
                &rows,
                crate::daemon::fsevents::Stream::current_event_id(),
                &sampling,
            ),
            Some(_) => None,
        };
        Ok(HostRead {
            busy: judge(&host.name, &rows, &cwds, &sampling, dashboard),
            placement_diagnostic,
            wedged,
            hung: hung_tests(&host.name, &rows, &cwds, &sampling),
            zombie_parents: crate::proc_limit::zombie_parents(&scan.rows, &sampling),
        })
    })();
    HostBusy {
        host: host.name.clone(),
        read,
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
        match &host.read {
            Err(error) => out.push(format!("    {}: not read: {error}", host.host)),
            Ok(read) if read.busy.is_empty() => out.push(format!("    {}: none", host.host)),
            Ok(read) => out.extend(read.busy.iter().map(Busy::line)),
        }
    }
    out
}

/// The "needs action" lines for <hosts>.
pub fn findings(hosts: &[HostBusy]) -> Vec<Advice> {
    let mut out = Vec::new();
    for read in hosts.iter().filter_map(|host| host.read.as_ref().ok()) {
        // A wedged fseventsd gets its own finding, in place of the busy one.
        out.extend(read.wedged.iter().map(|w| w.finding.clone()));
        let skip = read.wedged.as_ref().map(|w| w.pid);
        out.extend(
            read.busy
                .iter()
                .filter(|busy| Some(busy.pid) != skip)
                .filter_map(Busy::finding),
        );
        out.extend(read.hung.iter().map(Hung::finding));
    }
    out
}

#[cfg(test)]
#[path = "stuck_tests.rs"]
mod tests;
