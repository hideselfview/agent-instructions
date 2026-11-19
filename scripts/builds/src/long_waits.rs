//! Work that has run long, on this Mac and on every host, and what each piece
//! of it waits on: a `builds install` that waits at the gates it closed
//! (install_gate.rs), since every command that starts while it waits waits
//! too; a hook run (`builds checks run`), a landing or a `builds cargo` on
//! this Mac past its limit; and a cargo job on a host (remote.rs) past its
//! limit whose caller here runs (orphans.rs lists one whose caller is gone).
//! `builds health` lists each under "needs action".
//!
//! What a command on this Mac waits on is what runs under it: the jobs its
//! processes run on a host (a job's run id names the `builds` process here
//! that runs it), else the `builds` commands under it, else its other
//! processes. What a job waits on is what runs under its process group on
//! the host: compiles that sccache runs, compilers, test binaries. A line
//! names a command's tree once: a hook run that an install waits for, or a
//! job under a hook run that is named, gets no line of its own.

use crate::health::Advice;
use crate::host::{Host, place};
use crate::host_scan::{JobRun, Scan, arguments, program, tree};
use crate::install_gate::Waiting;
use crate::shells::shown;
use crate::stuck::Row;
use crate::text::duration;

/// How long each kind of work may run before health names it, in minutes.
/// A hook run's checks aim to finish in 45 seconds; an install that waits
/// at closed gates holds up every command that starts meanwhile.
const HOOK_MINUTES: i64 = 10;
const LAND_MINUTES: i64 = 30;
const CARGO_MINUTES: i64 = 30;
const JOB_MINUTES: i64 = 30;
const INSTALL_MINUTES: i64 = 2;

/// What a `builds` command on this Mac is called in a line, and how long it
/// may run; None for a command health does not time.
fn limit_of(row: &Row) -> Option<(&'static str, i64)> {
    if program(row) != "builds" {
        return None;
    }
    match arguments(row)[..] {
        ["checks", "run", ..] => Some(("hook run", HOOK_MINUTES)),
        ["land", ..] => Some(("landing", LAND_MINUTES)),
        ["cargo", ..] => Some(("cargo run", CARGO_MINUTES)),
        _ => None,
    }
}

/// What health read: this Mac's processes, and each host that answered with
/// its scan, this Mac among them.
struct Read<'a> {
    local: &'a [Row],
    hosts: Vec<(&'a Host, &'a Scan)>,
    /// The checkout a process on this Mac works in, from its working folder.
    checkout: &'a dyn Fn(i32) -> Option<String>,
    now: i64,
}

impl Read<'_> {
    /// `builds checks run (pid 12, in bae/side-3)`.
    fn named(&self, row: &Row) -> String {
        let checkout = (self.checkout)(row.pid)
            .map(|checkout| format!(", in {checkout}"))
            .unwrap_or_default();
        format!(
            "{} (pid {}{checkout})",
            shown(&format!("builds {}", arguments(row).join(" "))),
            row.pid
        )
    }

    /// What runs under process <pid> on this Mac, as the words after "it
    /// waits on".
    fn waits_on(&self, pid: i32) -> String {
        let under = tree(self.local, pid);
        let pids: Vec<i32> = under.iter().map(|row| row.pid).collect();
        let jobs: Vec<String> = self
            .hosts
            .iter()
            .flat_map(|(host, scan)| {
                scan.jobs
                    .iter()
                    .filter(|job| job.caller().is_some_and(|caller| pids.contains(&caller)))
                    .map(|job| self.job(host, scan, job))
            })
            .collect();
        if !jobs.is_empty() {
            return format!("it waits on {}", jobs.join("; and on "));
        }
        let below = &under[under.len().min(1)..];
        let builds: Vec<String> = below
            .iter()
            .filter(|row| program(row) == "builds")
            .map(|row| self.named(row))
            .collect();
        if !builds.is_empty() {
            return format!("it waits on {}", builds.join(", "));
        }
        let leaves: Vec<String> = below
            .iter()
            .filter(|row| !below.iter().any(|child| child.ppid == row.pid))
            .take(3)
            .map(|row| {
                format!(
                    "{} (pid {}, {})",
                    shown(&row.command),
                    row.pid,
                    duration(row.age_seconds)
                )
            })
            .collect();
        if leaves.is_empty() {
            "nothing runs under it".to_string()
        } else {
            format!("it waits on {}", leaves.join(", "))
        }
    }

    /// `its job on host m1 in bae/remote-side-3, which has run 36m: 14
    /// compiles through sccache`.
    fn job(&self, host: &Host, scan: &Scan, job: &JobRun) -> String {
        let age = job
            .started()
            .map(|started| format!(", which has run {}", duration(self.now - started)))
            .unwrap_or_default();
        format!(
            "its job on {} in {}{age}: {}",
            place(&host.name),
            job.label(),
            under_job(scan, job)
        )
    }
}

/// What runs under <job>'s process group leader on its host.
fn under_job(scan: &Scan, job: &JobRun) -> String {
    let under = tree(&scan.rows, job.leader);
    let clients = under
        .iter()
        .filter(|row| program(row) == "sccache" && !arguments(row).is_empty())
        .count();
    let compilers = under
        .iter()
        .filter(|row| matches!(program(row), "rustc" | "clippy-driver" | "rustdoc"))
        .count();
    let mut parts = Vec::new();
    if clients > 0 {
        parts.push(crate::text::counted(
            clients,
            "compile through sccache",
            "compiles through sccache",
        ));
    }
    if compilers > 0 {
        parts.push(crate::text::counted(compilers, "compiler", "compilers"));
    }
    for test in under.iter().filter(|row| crate::stuck::is_test_binary(row)) {
        parts.push(format!(
            "test binary {} (pid {}, {})",
            program(test),
            test.pid,
            duration(test.age_seconds)
        ));
    }
    if parts.is_empty() {
        "no compile or test runs under it".to_string()
    } else {
        parts.join(", ")
    }
}

/// The lines for the work that has run long, given each host's <scans> (this
/// Mac's among them), what a running install waits for (`install`), the
/// checkout each process on this Mac works in (`checkout`), and the time
/// <now>, in seconds since 1970 on this Mac's clock.
pub fn findings(
    scans: &[(Host, std::result::Result<Scan, String>)],
    install: &std::result::Result<Option<Waiting>, String>,
    checkout: &dyn Fn(i32) -> Option<String>,
    now: i64,
) -> Vec<Advice> {
    let hosts: Vec<(&Host, &Scan)> = scans
        .iter()
        .filter_map(|(host, scan)| Some((host, scan.as_ref().ok()?)))
        .collect();
    // stuck.rs lists a host whose scan failed, so a line here would repeat it.
    let Some(local) = hosts
        .iter()
        .find(|(host, _)| host.target.is_none())
        .map(|(_, scan)| scan.rows.as_slice())
    else {
        return Vec::new();
    };
    let read = Read {
        local,
        hosts: hosts.clone(),
        checkout,
        now,
    };
    let mut found = Vec::new();
    // The processes on this Mac whose trees a line already names.
    let mut named: Vec<i32> = Vec::new();
    let name_tree = |pid: i32, named: &mut Vec<i32>| {
        named.extend(tree(local, pid).iter().map(|row| row.pid));
    };
    match install {
        Err(error) => found.push(Advice::said(format!(
            "install: health could not read whether a builds install waits: {error}"
        ))),
        Ok(None) => {}
        Ok(Some(waiting)) => {
            let installer = local
                .iter()
                .find(|row| program(row) == "builds" && arguments(row).first() == Some(&"install"));
            if installer.is_none_or(|row| row.age_seconds >= INSTALL_MINUTES * 60) {
                let who = match installer {
                    Some(row) => format!(
                        "builds install (pid {}) has run {}",
                        row.pid,
                        duration(row.age_seconds)
                    ),
                    None => "a builds install runs".to_string(),
                };
                let held: Vec<String> = waiting
                    .held
                    .iter()
                    .map(|held| {
                        name_tree(held.pid, &mut named);
                        format!("{} ({})", held.text, read.waits_on(held.pid))
                    })
                    .collect();
                let what = if held.is_empty() {
                    "with its gates closed".to_string()
                } else {
                    format!("and waits for {}", held.join("; and for "))
                };
                let held_up = if waiting.connections_closed {
                    "every builds command that opens a store waits for it"
                } else {
                    "every hook run, review and app build lane that starts waits for it"
                };
                found.push(Advice::said(format!("install: {who} {what}; {held_up}")));
            }
        }
    }
    let mut long: Vec<(&Row, &str, i64)> = local
        .iter()
        .filter_map(|row| {
            let (kind, limit) = limit_of(row)?;
            (row.age_seconds >= limit * 60).then_some((row, kind, limit))
        })
        .collect();
    // The oldest first: a command started before the commands it runs.
    long.sort_by_key(|(row, _, _)| std::cmp::Reverse(row.age_seconds));
    for (row, kind, limit) in long {
        if named.contains(&row.pid) {
            continue;
        }
        name_tree(row.pid, &mut named);
        found.push(Advice::said(format!(
            "{kind}: {} has run {}, longer than {limit}m; {}",
            read.named(row),
            duration(row.age_seconds),
            read.waits_on(row.pid)
        )));
    }
    for (host, scan) in &hosts {
        for job in scan.jobs.iter().filter(|job| job.is_cargo_job()) {
            let Some(started) = job.started() else {
                continue;
            };
            if now - started < JOB_MINUTES * 60
                || job.caller().is_some_and(|caller| named.contains(&caller))
            {
                continue;
            }
            // orphans.rs lists a job whose caller here is gone.
            let Some(caller) = job
                .caller()
                .and_then(|caller| local.iter().find(|row| row.pid == caller))
            else {
                continue;
            };
            let caller = format!("for {} on this Mac", read.named(caller));
            found.push(Advice::said(format!(
                "job: {} has run a job in {} for {}, longer than {JOB_MINUTES}m, {caller}: {}",
                place(&host.name),
                job.label(),
                duration(now - started),
                under_job(scan, job)
            )));
        }
    }
    found
}

#[cfg(test)]
#[path = "long_waits_tests.rs"]
mod tests;
