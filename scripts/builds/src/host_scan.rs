//! One reading of a host as a whole, for `builds health` and the dashboard:
//! its processes, how much of its CPU sits idle, the commands that builds'
//! jobs run there, and its sccache server. One script prints them, over the
//! ssh builds already uses for hosts or with sh on this Mac, so a health
//! check asks each host once. stuck.rs judges the processes, host_load.rs the
//! idle share, long_waits.rs the jobs, orphans.rs the processes whose caller
//! is gone and sccache_queue.rs the server.

use crate::fail;
use crate::fail::Result;
use crate::host::{Accept, Host, Ssh, Terminal, command_at, output_of, place};
use crate::stuck::{PS_FORMAT, Row, parse_ps};

/// The line after which the script prints its `ps` listing: every line
/// before it is `<key>=<value>`, and no `ps` line can equal it, since each
/// starts with a pid.
const PS_FOLLOWS: &str = "ps_follows=1";

/// What a host's script printed, read.
#[derive(Clone, Debug)]
pub struct Scan {
    /// The user the script ran as: the user builds' jobs run as there.
    pub uid: u32,
    pub rows: Vec<Row>,
    /// The share of the host's CPU that sat idle over one second, as a
    /// percentage, or why the script could not read it.
    pub cpu_idle: std::result::Result<f64, String>,
    /// The commands builds' jobs run there now (remote.rs).
    pub jobs: Vec<JobRun>,
    /// The sccache server that listens there, or None when none does; Err
    /// when the script could not tell.
    pub sccache: std::result::Result<Option<CacheServer>, String>,
}

/// A host's sccache server and what it says about its compiles.
#[derive(Clone, Debug, PartialEq)]
pub struct CacheServer {
    pub pid: i32,
    pub stats: CacheStats,
}

/// What a server's JSON stats say, since it started.
#[derive(Clone, Debug, PartialEq)]
pub struct CacheStats {
    /// The Rust compiles the cache answered, and those it had to run.
    pub rust_hits: u64,
    pub rust_misses: u64,
    /// The bytes in the cache, and the most it keeps.
    pub cache_size: u64,
    pub max_cache_size: u64,
}

/// Reads a server's JSON stats <json>. Fails, with what the server printed,
/// when a field is missing. A server that has run no Rust compile has no
/// `Rust` count, which reads as 0.
pub fn parse_stats(json: &str) -> Result<CacheStats> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(error) => fail!("sccache printed stats that are not JSON ({error}): {json}"),
    };
    if !value["stats"]["cache_hits"].is_object() || !value["stats"]["cache_misses"].is_object() {
        fail!("sccache's stats have no counts of hits and misses: {json}");
    }
    let rust = |kind: &str| {
        value["stats"][kind]["counts"]["Rust"]
            .as_u64()
            .unwrap_or_default()
    };
    let size = |key: &str| -> Result<u64> {
        match value[key].as_u64() {
            Some(size) => Ok(size),
            None => fail!("sccache's stats have no {key}: {json}"),
        }
    };
    Ok(CacheStats {
        rust_hits: rust("cache_hits"),
        rust_misses: rust("cache_misses"),
        cache_size: size("cache_size")?,
        max_cache_size: size("max_cache_size")?,
    })
}

/// A command that a job runs on a host now: the claim folder of a worktree
/// (`<worktree>.job`) whose `pid` file names a process group that still
/// runs (`remote::run_script`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRun {
    /// The worktree the command runs in, on the host.
    pub worktree: String,
    /// The pid of the command's process group leader, on the host.
    pub leader: i32,
    /// The run's id (`remote::job_id`): `<pid>-<nanoseconds>`, the `builds`
    /// process on this Mac that runs the command and when it made the id.
    pub run: String,
}

impl JobRun {
    /// The pid on this Mac of the `builds` process that runs the command.
    pub fn caller(&self) -> Option<i32> {
        self.run.split_once('-')?.0.parse().ok()
    }

    /// When the command started, in seconds since 1970 on this Mac's clock.
    pub fn started(&self) -> Option<i64> {
        let nanos: u128 = self.run.split_once('-')?.1.parse().ok()?;
        i64::try_from(nanos / 1_000_000_000).ok()
    }

    /// The worktree's folder name: `remote-side-3`, `mac-1`.
    pub fn name(&self) -> &str {
        self.worktree.rsplit('/').next().unwrap_or(&self.worktree)
    }

    /// `bae/remote-side-3` (`stuck::checkout_label`), or the worktree's path
    /// when it is in no checkout's place.
    pub fn label(&self) -> String {
        crate::stuck::checkout_label(&self.worktree).unwrap_or_else(|| self.worktree.clone())
    }

    /// Whether the worktree is one that `builds cargo` and `builds remote
    /// run` send jobs to (`remote::JOB_WORKTREE_PREFIX`), not an app
    /// checkout.
    pub fn is_cargo_job(&self) -> bool {
        self.name().starts_with(crate::remote::JOB_WORKTREE_PREFIX)
    }
}

/// The script a host runs. `top` takes two samples a second apart, and the
/// second one says what the CPU did in that second; the first counts from
/// boot. A job's claim folder whose `pid` file names a group that no longer
/// runs is a job that ended, or an app checkout between steps, so the script
/// prints only the groups that run.
pub fn script(host: &Host) -> String {
    format!(
        r#"{root}echo "uid=$(id -u)"
echo "cpu=$(top -l 2 -n 0 -s 1 2>&1 | awk '/^CPU usage/{{line=$0}} END{{print line}}')"
for claim in "$HOME"/dev/*-worktrees/*.job; do
  [ -f "$claim/pid" ] || continue
  set -- $(cat "$claim/pid" 2>/dev/null)
  case "$1" in ''|*[!0-9]*) continue;; esac
  kill -0 -- "-$1" 2>/dev/null && echo "job=${{claim%.job}} $1 $2"
done
{listener}
echo {PS_FOLLOWS}
ps -axww -o {PS_FORMAT}"#,
        root = crate::remote::root_script(host),
        listener = crate::sccache::listener_script(),
    )
}

/// The idle share in top's line `CPU usage: 84.8% user, 15.1% sys, 0.0%
/// idle`.
fn parse_cpu(line: &str) -> std::result::Result<f64, String> {
    line.split(',')
        .find_map(|part| part.trim().strip_suffix("% idle")?.trim().parse().ok())
        .ok_or_else(|| format!("top printed no idle share: {line:?}"))
}

/// Reads what `script` printed.
pub fn parse(text: &str) -> std::result::Result<Scan, String> {
    let Some((head, ps)) = text.split_once(&format!("\n{PS_FOLLOWS}\n")) else {
        return Err(format!("the host printed no process listing: {text:?}"));
    };
    let value = |key: &str| crate::host::answer_value(head, key);
    let jobs = head
        .lines()
        .filter_map(|line| {
            let (rest, run) = line.strip_prefix("job=")?.rsplit_once(' ')?;
            let (worktree, leader) = rest.rsplit_once(' ')?;
            Some(JobRun {
                worktree: worktree.to_string(),
                leader: leader.parse().ok()?,
                run: run.to_string(),
            })
        })
        .collect();
    let uid = value("uid")
        .and_then(|uid| uid.parse().ok())
        .ok_or_else(|| format!("the host did not say its user id: {head:?}"))?;
    let sccache = (|| -> Result<Option<CacheServer>> {
        let Some(pid) = crate::sccache::parse_listener_lines(head)? else {
            return Ok(None);
        };
        let Some(json) = value("sccache_stats") else {
            fail!("the sccache server listens, but the host printed no stats");
        };
        Ok(Some(CacheServer {
            pid: pid as i32,
            stats: parse_stats(json)?,
        }))
    })()
    .map_err(|fault| fault.text());
    Ok(Scan {
        uid,
        rows: parse_ps(ps)?,
        cpu_idle: parse_cpu(value("cpu").unwrap_or_default()),
        jobs,
        sccache,
    })
}

/// Runs the script on <host> and reads what it printed.
pub fn read(ssh: &Ssh, host: &Host) -> std::result::Result<Scan, String> {
    let text = output_of(
        command_at(ssh, host, &script(host), Terminal::None),
        &format!("reading {}", place(&host.name)),
        Accept::Zero,
    )?;
    parse(&text)
}

/// The rows of <rows> under <pid>, it included, nearest first.
pub fn tree(rows: &[Row], pid: i32) -> Vec<&Row> {
    let mut found: Vec<&Row> = rows.iter().filter(|row| row.pid == pid).collect();
    let mut at = 0;
    while at < found.len() {
        let parent = found[at].pid;
        for row in rows.iter().filter(|row| row.ppid == parent) {
            if !found.iter().any(|seen| seen.pid == row.pid) {
                found.push(row);
            }
        }
        at += 1;
    }
    found
}

/// The file name of <row>'s program: `cargo` for `/x/bin/cargo test`.
pub fn program(row: &Row) -> &str {
    crate::shells::basename(row.command.split_whitespace().next().unwrap_or(""))
}

/// The words of <row>'s command after its program.
pub fn arguments(row: &Row) -> Vec<&str> {
    row.command.split_whitespace().skip(1).collect()
}

#[cfg(test)]
#[path = "host_scan_tests.rs"]
mod tests;
