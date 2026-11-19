//! Processes whose caller is gone, on this Mac and on every host: nothing
//! waits for what they print, yet they hold cores, locks and claims.
//!
//! - A development tool (cargo, a compiler, an sccache client, a test binary
//!   cargo built, git, xcodebuild, builds' own ssh to a host, or `builds`)
//!   whose parent is launchd: the process that started it ended, and
//!   launchd took it over. builds starts on purpose only the dashboard, the
//!   prune of build copies after a build (`builds units`), the audit a
//!   landing's review samples (`builds review audit`) and the sccache server
//!   that way, and git starts its daemons that way, so none of those is one.
//! - A job's command on a host (host_scan.rs) whose `builds` process on this
//!   Mac is gone, judged as job_claim.rs judges a claim.
//!
//! `builds health` stops each one whose only work is what it prints (a cargo
//! run, a hook run whose commit is gone, a compile, a test, a job), with what
//! runs under it, in its fix pass (health_fix.rs), and lists the rest under
//! "needs action": a `builds` command such as a landing or an install still
//! does its work when no one reads it.

use crate::health::Advice;
use crate::host::{Host, place};
use crate::host_scan::{JobRun, Scan, arguments, program, tree};
use crate::job_claim::{Holder, Holders};
use crate::shells::shown;
use crate::stuck::Row;
use crate::text::duration;

/// A process younger than this may be on its way out with its parent.
const ORPHAN_SECONDS: i64 = 60;

/// The pid of launchd, which takes over a process whose parent ended.
const LAUNCHD: i32 = 1;

/// The development tools that no one starts under launchd on purpose.
const TOOLS: [&str; 8] = [
    "cargo",
    "cargo-clippy",
    "cargo-nextest",
    "rustc",
    "clippy-driver",
    "rustdoc",
    "git",
    "xcodebuild",
];

/// What a process whose caller is gone is.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A process whose parent is launchd, and the pids of it and every
    /// process under it, it first.
    Process {
        row: Row,
        tree: Vec<i32>,
        /// Whether its only work is what it prints, so stopping it loses
        /// nothing.
        stops: bool,
    },
    /// A job's command on a host whose `builds` process on this Mac is gone.
    Job { job: JobRun },
}

/// A process, or a job's process group, whose caller is gone.
#[derive(Clone, Debug, PartialEq)]
pub struct Orphan {
    pub host: Host,
    pub kind: Kind,
}

impl Orphan {
    /// The script that stops it on its host, or None when health leaves it
    /// to run.
    pub fn stop_script(&self) -> Option<String> {
        match &self.kind {
            Kind::Process { tree, stops, .. } => stops.then(|| {
                let pids: Vec<String> = tree.iter().map(i32::to_string).collect();
                format!("kill -TERM {}", pids.join(" "))
            }),
            Kind::Job { job } => Some(format!("kill -TERM -- -{}", job.leader)),
        }
    }

    /// The stop script as the person runs it: over ssh on a host.
    fn stop_command(&self) -> Option<String> {
        let script = self.stop_script()?;
        Some(match &self.host.target {
            Some(target) => format!("ssh {target} '{script}'"),
            None => script,
        })
    }

    /// What it is, for a line: `host m1 pid 12 (cargo test), 3h05m old`.
    pub fn what(&self) -> String {
        let place = place(&self.host.name);
        match &self.kind {
            Kind::Process { row, tree, .. } => format!(
                "{place} pid {} ({}), {} old{}",
                row.pid,
                shown(&row.command),
                duration(row.age_seconds),
                match tree.len() {
                    0 | 1 => String::new(),
                    n => format!(
                        ", with {} under it",
                        crate::text::counted(n - 1, "process", "processes")
                    ),
                }
            ),
            Kind::Job { job } => format!(
                "the job in {} on {place} (process group {}), which process {} on this Mac ran",
                job.label(),
                job.leader,
                job.caller().unwrap_or_default()
            ),
        }
    }

    /// The line under "needs action".
    pub fn finding(&self) -> Advice {
        let what = self.what();
        match self.stop_command() {
            Some(command) => Advice::run(
                format!(
                    "orphan: {what}: its caller is gone, so nothing reads what it prints; stop it with"
                ),
                command,
            ),
            None => Advice::said(format!(
                "orphan: {what}: its caller is gone; it does its work to its end, and nothing reads what it prints"
            )),
        }
    }
}

/// Whether <row>, whose parent is launchd, is a tool whose caller is gone,
/// and whether its only work is what it prints; None for a process that
/// runs under launchd on purpose. <targets> are the hosts' ssh targets.
fn judge(row: &Row, targets: &[&str]) -> Option<bool> {
    let words = arguments(row);
    match program(row) {
        "builds" => match words[..] {
            [] | ["dashboard", ..] | ["units", ..] | ["review", "audit", ..] => None,
            ["cargo", ..] | ["remote", ..] | ["checks", "run", ..] => Some(true),
            _ => Some(false),
        },
        "git" => (!words.iter().any(|word| word.ends_with("--daemon"))).then_some(true),
        "sccache" => (!words.is_empty()).then_some(true),
        "ssh" => (words.contains(&"BatchMode=yes")
            && words.iter().any(|word| targets.contains(word)))
        .then_some(true),
        tool if TOOLS.contains(&tool) => Some(true),
        _ => crate::stuck::is_test_binary(row).then_some(true),
    }
}

/// The processes whose caller is gone among each host's <scans>, given this
/// Mac's processes as <holders> read them before the scans.
pub fn find(scans: &[(Host, std::result::Result<Scan, String>)], holders: &Holders) -> Vec<Orphan> {
    let targets: Vec<&str> = scans
        .iter()
        .filter_map(|(host, _)| host.target.as_deref())
        .collect();
    let mut found = Vec::new();
    for (host, scan) in scans {
        let Ok(scan) = scan else {
            continue;
        };
        for row in scan.rows.iter().filter(|row| {
            // A zombie has ended: only its parent's reap removes it.
            row.ppid == LAUNCHD
                && row.uid == scan.uid
                && row.age_seconds >= ORPHAN_SECONDS
                && !row.zombie
        }) {
            if let Some(stops) = judge(row, &targets) {
                found.push(Orphan {
                    host: host.clone(),
                    kind: Kind::Process {
                        row: row.clone(),
                        tree: tree(&scan.rows, row.pid)
                            .iter()
                            .map(|row| row.pid)
                            .collect(),
                        stops,
                    },
                });
            }
        }
        for job in &scan.jobs {
            // A job whose leader is a process above is already listed.
            if found.iter().any(|orphan| {
                orphan.host.name == host.name
                    && matches!(&orphan.kind, Kind::Process { tree, .. } if tree.contains(&job.leader))
            }) {
                continue;
            }
            if holders.holder(&job.run) == Some(Holder::Gone) {
                found.push(Orphan {
                    host: host.clone(),
                    kind: Kind::Job { job: job.clone() },
                });
            }
        }
    }
    found
}

#[cfg(test)]
#[path = "orphans_tests.rs"]
mod tests;
