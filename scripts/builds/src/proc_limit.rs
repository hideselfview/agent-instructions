//! Whether a host nears its limit of processes per user. A process that exits
//! stays in the process table as a zombie until its parent reaps it, and a
//! parent that never does fills the table: at the limit (`kern.maxprocperuid`)
//! every fork on the host fails and every command there fails with "fork
//! failed: resource temporarily unavailable".
//!
//! The probe reads the user's process count and the limit on every host,
//! this Mac included (`PROBE_LINES`, which `host.rs`'s probe script runs).
//! `builds health` lists under "needs action" each host
//! whose count passes `WARN_SHARE` of the limit, with the parents that hold the
//! most zombie children, which stuck.rs's `ps` listing shows. `stop` ends such
//! a parent on this Mac when it is a test binary, under `builds health
//! --stop-junk`.

use std::collections::HashMap;

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::health::Advice;
use crate::host::{Host, Ssh, answer_value, place};
use crate::host_scan::Row;
use crate::shells::{basename, shown};
use crate::text::duration;

/// A host above this share of its process limit needs action.
pub const WARN_SHARE: f64 = 0.6;
/// How many zombie parents a finding names.
const PARENTS: usize = 3;

/// Prints `procs=<the user's processes, zombies included>` (or
/// `procs_fails=<what ps said>` when `ps` fails) and `max_procs=<the per-user
/// limit>`, for `parse`. Linux keeps the limit in the shell's own limits (its
/// soft "Max processes"), or, when that is unlimited, in the kernel's
/// threads-max; dash's ulimit has no option that prints it.
pub const PROBE_LINES: &str = r#"if listed=$(ps -U $(id -u) -o pid= 2>&1); then echo procs=$(printf '%s\n' "$listed" | grep -c .); else echo "procs_fails=$(printf '%s' "$listed" | tr '\n' ' ')"; fi
case "$(uname -s)" in
Darwin) echo max_procs=$(sysctl -n kern.maxprocperuid) ;;
*) max=$(awk '/^Max processes/{print $3}' /proc/self/limits); [ "$max" = unlimited ] && max=$(cat /proc/sys/kernel/threads-max); echo max_procs=$max ;;
esac"#;

/// A host's process count and per-user limit at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessUse {
    pub count: u64,
    pub limit: u64,
}

impl ProcessUse {
    pub fn share(&self) -> f64 {
        self.count as f64 / self.limit.max(1) as f64
    }

    pub fn near_limit(&self) -> bool {
        self.limit > 0 && self.share() >= WARN_SHARE
    }
}

/// Reads what `PROBE_LINES` printed.
pub fn parse(text: &str) -> Result<ProcessUse> {
    if let Some(said) = answer_value(text, "procs_fails") {
        fail!("the host could not list its processes: {said}");
    }
    let value = |key: &str| -> Result<u64> {
        match answer_value(text, key) {
            Some(value) => value
                .parse()
                .context(|| format!("reading the host's {key}")),
            None => fail!("the host's answer has no {key}: {text:?}"),
        }
    };
    Ok(ProcessUse {
        count: value("procs")?,
        limit: value("max_procs")?,
    })
}

/// A process that has zombie children.
#[derive(Clone, Debug, PartialEq)]
pub struct ZombieParent {
    pub pid: i32,
    pub uid: u32,
    pub command: String,
    pub age_seconds: i64,
    pub zombies: usize,
    /// A test binary cargo built: stopping it is safe for builds to do.
    pub test_binary: bool,
}

/// The processes of <rows> with the most zombie children, most
/// first. A zombie whose parent is not in <rows> (init, or another user's
/// process) counts for no parent: ending init's children is no fix.
fn zombie_parents(rows: &[Row]) -> Vec<ZombieParent> {
    let mut counts: HashMap<i32, usize> = HashMap::new();
    for zombie in rows.iter().filter(|row| row.zombie) {
        *counts.entry(zombie.ppid).or_default() += 1;
    }
    let mut parents: Vec<ZombieParent> = rows
        .iter()
        .filter(|row| row.pid > 1 && !row.zombie)
        .filter_map(|row| {
            let zombies = *counts.get(&row.pid)?;
            Some(ZombieParent {
                pid: row.pid,
                uid: row.uid,
                command: row.command.clone(),
                age_seconds: row.age_seconds,
                zombies,
                test_binary: row.is_test_binary(),
            })
        })
        .collect();
    parents.sort_by_key(|parent| (std::cmp::Reverse(parent.zombies), parent.pid));
    parents.truncate(PARENTS);
    parents
}

fn parent_text(parent: &ZombieParent) -> String {
    format!(
        "pid {} ({}, {} old) holds {} zombie children",
        parent.pid,
        shown(basename(
            parent.command.split_whitespace().next().unwrap_or("")
        )),
        duration(parent.age_seconds),
        parent.zombies
    )
}

fn head_of(host: &Host, used: &ProcessUse) -> String {
    format!(
        "processes: {} runs {} of its {} processes per user ({:.0}%), and every fork fails at the limit",
        place(&host.name),
        used.count,
        used.limit,
        used.share() * 100.0
    )
}

/// What to do when a host nears its process limit. The original scan names
/// the parents that hold the most zombies.
pub fn finding(
    host: &Host,
    used: ProcessUse,
    rows: std::result::Result<&[Row], &str>,
) -> Option<Advice> {
    if !used.near_limit() {
        return None;
    }
    let head = head_of(host, &used);
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            return Some(Advice::said(format!(
                "{head}; reading its processes failed, so the parents of zombies are unknown: {error}"
            )));
        }
    };
    let parents = zombie_parents(rows);
    Some(match parents.first() {
        None => Advice::said(format!(
            "{head}; no process there holds zombie children, so read `ps` on it for what holds the rest"
        )),
        Some(top) => {
            let list = parents
                .iter()
                .map(parent_text)
                .collect::<Vec<_>>()
                .join("; ");
            if top.test_binary {
                Advice::run(
                    format!(
                        "{head}; the top parents of zombies are {list}; stop the first, a test binary, with"
                    ),
                    crate::host::person_command(
                        host.target.as_deref(),
                        &format!("kill {}", top.pid),
                    ),
                )
            } else {
                Advice::said(format!(
                    "{head}; the top parents of zombies are {list}; the first is not a test binary, so look at what it is before stopping it"
                ))
            }
        }
    })
}

/// Stops each test-binary parent of zombies of <uid> on this Mac when this
/// Mac is near its process limit, with SIGTERM: the zombies then belong to
/// init, which reaps them. Returns one line per parent and the number it
/// could not stop. <ssh> probes this Mac for the count.
pub fn stop(ssh: &Ssh, uid: u32) -> Result<(Vec<String>, usize)> {
    let used = crate::host::read_probe(ssh, None)
        .map_err(Fail::Message)?
        .processes;
    if !used.near_limit() {
        return Ok((Vec::new(), 0));
    }
    let rows = crate::host_scan::local_rows().map_err(Fail::Message)?;
    let mut lines = Vec::new();
    let mut failed = 0;
    for parent in zombie_parents(&rows) {
        if !parent.test_binary || parent.uid != uid {
            continue;
        }
        if unsafe { libc::kill(parent.pid, libc::SIGTERM) } == 0 {
            lines.push(format!(
                "stopped pid {}, a test binary with {} zombie children: {}",
                parent.pid, parent.zombies, parent.command
            ));
        } else {
            failed += 1;
            lines.push(format!(
                "could not stop pid {}: {}",
                parent.pid,
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok((lines, failed))
}

#[cfg(test)]
#[path = "proc_limit_tests.rs"]
mod tests;
