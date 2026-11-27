//! Processes whose caller is gone, on this Mac and on every host: nothing
//! waits for what they print, yet they hold cores, locks and claims.
//!
//! - A development tool (cargo, a compiler, an sccache client, a test binary
//!   cargo built, git, xcodebuild, builds' own ssh to a host, or `builds`)
//!   whose parent is launchd: the process that started it ended, and
//!   launchd took it over. A process that builds detached, and what runs
//!   under it, is none: its host has a record of it (process.rs). Nor are
//!   git's daemons and the sccache server, which run that way on purpose and
//!   which builds does not detach.
//! - A job's command on a host (host_scan.rs) whose `builds` process on this
//!   Mac is gone, judged as job_claim.rs judges a claim.
//!
//! `builds health` stops each one whose only work is what it prints (a cargo
//! run, a hook run whose commit is gone, a compile, a test, a job), with what
//! runs under it, in its fix pass (health_fix.rs), and lists the rest under
//! "needs action": a `builds` command such as a landing or an install still
//! does its work when no one reads it.

use std::collections::BTreeSet;

use crate::command::Command;
use crate::health::Advice;
use crate::host::{Host, place};
use crate::host_scan::Row;
use crate::host_scan::{JobRun, Scan, arguments, program, tree, trees};
use crate::job_claim::{Holder, Holders};
use crate::shells::shown;
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
    /// A job's command on a host whose `builds` process on this Mac, the
    /// one with pid <caller>, is gone.
    Job { job: JobRun, caller: i32 },
}

/// A process, or a job's process group, whose caller is gone.
#[derive(Clone, Debug, PartialEq)]
pub struct Orphan {
    pub host: Host,
    pub kind: Kind,
}

impl Orphan {
    /// The `kill` arguments that stop it on its host (its pid and every pid
    /// under it, or its job's process group as `-<leader>`), or None when
    /// health leaves it to run.
    fn targets(&self) -> Option<Vec<String>> {
        match &self.kind {
            Kind::Process { tree, stops, .. } => {
                stops.then(|| tree.iter().map(i32::to_string).collect())
            }
            Kind::Job { job, .. } => Some(vec![format!("-{}", job.leader)]),
        }
    }

    /// Signals each target and reports any live process that remains.
    pub fn stop_script(&self) -> Option<String> {
        let targets = self.targets()?;
        let mut statements = Vec::new();
        for (i, target) in targets.iter().enumerate() {
            statements.push(format!(
                "if kill -TERM {target} 2>/dev/null; then signal_{i}='still runs after SIGTERM'; else signal_{i}='still runs because SIGTERM failed'; fi"
            ));
        }
        statements.push("procs=$(ps -axo pid=,stat=,pgid=) || exit 1".to_string());
        for (i, target) in targets.iter().enumerate() {
            let column = if target.starts_with('-') { "$3" } else { "$1" };
            let id = target.trim_start_matches('-');
            statements.push(format!(
                "live=$(printf '%s\\n' \"$procs\" | awk '{column} == {id} && $2 !~ /^Z/ {{ print 1; exit }}') || exit 1\n\
                 if [ -n \"$live\" ]; then echo \"{target} $signal_{i}\"; fi"
            ));
        }
        Some(statements.join("\n"))
    }

    /// The command that stops it, as the person runs it: over ssh on a host.
    fn stop_command(&self) -> Option<String> {
        let script = match &self.kind {
            Kind::Job { .. } => format!("kill -TERM {}", self.targets()?.join(" ")),
            Kind::Process { .. } => format!("kill -TERM {}", self.targets()?.join(" ")),
        };
        Some(crate::host::person_command(
            self.host.target.as_deref(),
            &script,
        ))
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
            Kind::Job { job, caller } => format!(
                "the job in {} on {place} (process group {}), which process {caller} on this Mac ran",
                job.label(),
                job.leader,
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

/// Whether <row>, whose parent is launchd and which builds did not detach,
/// is a tool whose caller is gone, and whether its only work is what it
/// prints; None for any other process. <targets> are the hosts' ssh targets.
fn judge(row: &Row, targets: &[&str]) -> Option<bool> {
    let words = arguments(row);
    match program(row) {
        "builds" => Some(
            Command::of(&words)
                .is_some_and(|parsed| parsed.command.orphaned() == Orphaned::OnlyPrints),
        ),
        "git" => (!words.iter().any(|word| word.ends_with("--daemon"))).then_some(true),
        "sccache" => (!words.is_empty()).then_some(true),
        "ssh" => (words.contains(&"BatchMode=yes")
            && words.iter().any(|word| targets.contains(word)))
        .then_some(true),
        tool if TOOLS.contains(&tool) => Some(true),
        _ => row.is_test_binary().then_some(true),
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
        let detached: BTreeSet<i32> = trees(&scan.rows, &scan.detached)
            .iter()
            .map(|row| row.pid)
            .collect();
        for row in scan.rows.iter().filter(|row| {
            // A zombie has ended: only its parent's reap removes it.
            !detached.contains(&row.pid)
                && row.ppid == LAUNCHD
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
                            .filter(|pid| !detached.contains(pid))
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
            // A run id that names a pid is one `holder` can judge.
            if let (Some(caller), Some(Holder::Gone)) = (job.caller(), holders.holder(&job.run)) {
                found.push(Orphan {
                    host: host.clone(),
                    kind: Kind::Job {
                        job: job.clone(),
                        caller,
                    },
                });
            }
        }
    }
    found
}

/// What a builds command does once its caller is gone (`Command::orphaned`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orphaned {
    /// Its only work is what it prints, which nothing reads now.
    OnlyPrints,
    /// It does its work to its end.
    Works,
}

impl crate::command::Command {
    /// What the command does once nothing reads it, as the orphan check
    /// (orphans.rs) judges a builds process whose caller is gone.
    pub fn orphaned(self) -> Orphaned {
        use crate::command::Command::*;
        use crate::command::*;
        match self {
            Cargo
            | Remote(remote::Run | remote::Step)
            | Checks(checks::Run)
            | Web(web::Picture) => Orphaned::OnlyPrints,
            Help
            | Ack
            | RulesOnEdit
            | Setup
            | Lake
            | Tool
            | New
            | Claim
            | Note
            | Release
            | Retire
            | AppBuild(
                app_build::Build
                | app_build::LandingCoverage
                | app_build::DropShots
                | app_build::RecordHanded,
            )
            | Lock
            | Programs
            | Queue
            | Takeover
            | AppClone
            | SccacheTmpdir
            | Prune
            | Du
            | Status
            | Health
            | InstallHook
            | Waits
            | Land
            | Brief
            | BuildApp
            | Dashboard(dashboard::Serve | dashboard::Stage)
            | Units(units::Prune | units::GitCheckouts | units::AfterBuild)
            | Install
            | Version
            | Next
            | Agent(agent::Start | agent::Run | agent::Resume | agent::List | agent::Show)
            | Shots(shots::Findings | shots::Picture | shots::CheckProgram | shots::Checker)
            | AppCheckout(app_checkout::Add | app_checkout::Remove | app_checkout::Reset)
            | Host(
                host::Add
                | host::Setup
                | host::SetupHere
                | host::SpeedHere
                | host::Band
                | host::List
                | host::Cache
                | host::Remove
                | host::ClearXcode,
            )
            | Checks(checks::Whole | checks::Stats | checks::List | checks::Tests)
            | Review(
                review::Run
                | review::Copy
                | review::Settle
                | review::Wait
                | review::Dispute
                | review::Keep
                | review::Show
                | review::Key
                | review::CheckLanding
                | review::Landed
                | review::Groups
                | review::Replay
                | review::Misses
                | review::Rates
                | review::Scores
                | review::Audit
                | review::CasePropose
                | review::CaseAdd,
            )
            | Judge(judge::Listen)
            | Question(
                question::Ask
                | question::Answer
                | question::Settle
                | question::Done
                | question::Drop
                | question::Place
                | question::Edit
                | question::Show
                | question::List,
            )
            | Item(
                item::Add
                | item::Edit
                | item::Note
                | item::Changes
                | item::Wait
                | item::Backlog
                | item::Start
                | item::Take
                | item::CheckBrief
                | item::Handoff
                | item::Stop
                | item::Finished
                | item::Drop
                | item::Place
                | item::CheckLanding
                | item::Landed
                | item::Show
                | item::List
                | item::Times,
            )
            | Area(area::Add | area::Rename | area::Move | area::Remove)
            | Decision(
                decision::Add | decision::Edit | decision::Find | decision::Show | decision::List,
            )
            | Design(
                design::Places
                | design::Only
                | design::Record
                | design::Amend
                | design::Approve
                | design::Check
                | design::Landed
                | design::Title
                | design::Drop
                | design::Show
                | design::List
                | design::Render
                | design::TokensCss
                | design::PublishFind
                | design::PublishFiles
                | design::PublishLast
                | design::Listen
                | design::PublishTake
                | design::PublishDone
                | design::PublishFailed,
            )
            | Lists(
                lists::Init | lists::DesignGlobs | lists::AddRepo | lists::Show | lists::ProjectOf,
            ) => Orphaned::Works,
        }
    }
}

#[cfg(test)]
#[path = "orphans_tests.rs"]
mod tests;
