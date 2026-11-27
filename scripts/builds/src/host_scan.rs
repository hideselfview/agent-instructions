//! One reading of a host as a whole, for `builds health` and the dashboard:
//! its processes, how much of its CPU sits idle, the commands that builds'
//! jobs run there, and its sccache server. One script prints them, over the
//! ssh builds already uses for hosts or with sh on this Mac, so a health
//! check asks each host once. stuck.rs judges the processes, host_load.rs the
//! idle share, health_stillness.rs the jobs, orphans.rs the processes whose caller
//! is gone and sccache_queue.rs the server.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::fail;
use crate::fail::Result;
use crate::host::{Host, Ssh, output_of, place};
use crate::process_readings::{self, Reading, measure};

/// The line after which the script prints its `ps` listing: every line
/// before it is `<key>=<value>`, and no `ps` line can equal it, since each
/// starts with a pid.
const PS_FOLLOWS: &str = "ps_follows=1";

/// What a host's script printed, read.
#[derive(Clone, Debug)]
pub struct Scan {
    /// The user the script ran as: the user builds' jobs run as there.
    pub uid: u32,
    /// Seconds since 1970 by the host's clock when it listed the processes.
    pub at: i64,
    pub rows: Vec<Row>,
    /// When each process last used CPU, by this host's clock.
    pub active: BTreeMap<i32, i64>,
    /// The share of the host's CPU that sat idle over one second, as a
    /// percentage, or why the script could not read it.
    pub cpu_idle: std::result::Result<f64, String>,
    /// The commands builds' jobs run there now (remote.rs).
    pub jobs: Vec<JobRun>,
    /// The sccache server that listens there, or None when none does; Err
    /// when the script could not tell.
    pub sccache: std::result::Result<Option<CacheServer>, String>,
    /// What the host's display reading says (host_display.rs): None on a
    /// host that is no Mac, and Err when the host's answer cannot be read.
    pub display: std::result::Result<Option<crate::host_display::Display>, String>,
}

/// A host's sccache server and what it says about its compiles.
#[derive(Clone, Debug, PartialEq)]
pub struct CacheServer {
    pub pid: i32,
    pub stats: crate::sccache::CacheStats,
}

/// A command that a job runs on a host now: the claim folder of a worktree
/// (`<worktree>.job`) whose run record (`remote::RUN_FILE`) names a process
/// group that still runs.
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

    /// `bae/remote-side-3` (`stuck::checkout_label`), or the worktree's path
    /// when it is in no checkout's place.
    pub fn label(&self) -> String {
        crate::stuck::checkout_label(&self.worktree).unwrap_or_else(|| self.worktree.clone())
    }
}

/// The script a host runs. On macOS `top` takes two samples a second apart,
/// and the second one says what the CPU did in that second; the first counts
/// from boot. On Linux the script reads the CPU's counters in /proc/stat a
/// second apart and prints the idle share of that second (`cpu_idle`): the
/// idle and the I/O-wait time, over the time of every state but the guests',
/// which the user time already counts. The run script removes a claim
/// folder's run record when its command ends, but one that was killed leaves
/// the record behind, naming a group that no longer runs, so the script
/// prints only the groups that run. dash, Linux's sh, takes no `--` before a
/// process group. A Mac also prints its display reading (host_display.rs).
pub fn script(host: &Host) -> String {
    format!(
        r#"{root}echo "uid=$(id -u)"
case "$(uname -s)" in
Darwin) echo "cpu=$(top -l 2 -n 0 -s 1 2>&1 | awk '/^CPU usage/{{line=$0}} END{{print line}}')"
{display}
;;
*) echo "cpu_idle=$( (head -n 1 /proc/stat; sleep 1; head -n 1 /proc/stat) | awk '{{t = 0; for (i = 2; i <= 9; i++) t += $i; if (NR == 1) {{ t1 = t; i1 = $5 + $6 }} else if (t > t1) printf "%.1f", ($5 + $6 - i1) * 100 / (t - t1)}}')" ;;
esac
for claim in "$HOME"/dev/*-worktrees/*.job; do
  [ -f "$claim/{run_file}" ] || continue
  set -- $(cat "$claim/{run_file}" 2>/dev/null)
  case "$1" in ''|*[!0-9]*) continue;; esac
  kill -0 "-$1" 2>/dev/null && echo "job=${{claim%.job}} $1 $2"
done
{listener}
{timed_ps}"#,
        root = crate::remote::root_script(host),
        listener = crate::sccache::listener_script(),
        timed_ps = timed_ps_script(),
        display = crate::host_display::SCRIPT,
        run_file = crate::remote::RUN_FILE,
    )
}

/// The idle share in top's line `CPU usage: 84.8% user, 15.1% sys, 0.0%
/// idle`.
fn parse_cpu(line: &str) -> std::result::Result<f64, String> {
    line.split(',')
        .find_map(|part| part.trim().strip_suffix("% idle")?.trim().parse().ok())
        .ok_or_else(|| format!("top printed no idle share: {line:?}"))
}

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

impl Row {
    /// `name-<16 hex digits>`, the file name of a test binary cargo builds.
    pub fn is_test_binary(&self) -> bool {
        let program = self.command.split_whitespace().next().unwrap_or("");
        crate::shells::basename(program)
            .rsplit_once('-')
            .is_some_and(|(name, hash)| {
                !name.is_empty() && hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit())
            })
            && program.contains("/target/")
    }
}

/// What `ps` prints, one process a line: pid, parent, user, CPU share, CPU
/// time, time since start, state, command with its arguments.
pub const PS_FORMAT: &str = "pid=,ppid=,uid=,pcpu=,cputime=,etime=,stat=,command=";

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
pub fn parse_ps(text: &str) -> std::result::Result<Vec<Row>, String> {
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

const PS_ARGS: [&str; 3] = ["-axww", "-o", PS_FORMAT];

/// The command to list every process with the fields the process reader uses.
pub fn ps_script() -> String {
    format!("ps {}", PS_ARGS.join(" "))
}

/// The rows of this Mac's processes, from one ps.
pub fn local_rows() -> std::result::Result<Vec<Row>, String> {
    let mut command = std::process::Command::new("ps");
    command.args(PS_ARGS);
    parse_ps(&output_of(command, "ps on this Mac")?)
}

/// The timestamp and the rows of a process listing, by the host's clock.
pub fn timed_ps_script() -> String {
    format!(
        r#"now=$(date +%s) || exit $?
echo "now=$now"
echo {PS_FOLLOWS}
{ps}"#,
        ps = ps_script()
    )
}

pub fn timed_rows(text: &str) -> std::result::Result<(&str, i64, Vec<Row>), String> {
    let (head, ps) = text
        .split_once(&format!("\n{PS_FOLLOWS}\n"))
        .ok_or_else(|| format!("the host printed no process listing: {text:?}"))?;
    let at = crate::host::answer_value(head, "now")
        .and_then(|at| at.parse().ok())
        .ok_or_else(|| format!("the host did not say what time it is: {head:?}"))?;
    Ok((head, at, parse_ps(ps)?))
}

/// Reads what `script` printed. Each row's CPU share is what the process
/// used since the reading of <readings> that `process_readings::measure`
/// compares with.
#[cfg(test)]
pub fn parse(text: &str, readings: &[Reading]) -> std::result::Result<Scan, String> {
    read_scan(text, readings).map(|(scan, _)| scan)
}

fn read_scan(text: &str, readings: &[Reading]) -> std::result::Result<(Scan, Reading), String> {
    let (head, at, rows) = timed_rows(text)?;
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
            stats: crate::sccache::parse_json_stats(json)?,
        }))
    })()
    .map_err(|fault| fault.text());
    let cpu = Reading::following(at, &rows, readings);
    let scan = Scan {
        uid,
        at,
        active: cpu
            .procs
            .iter()
            .filter_map(|p| Some((p.pid, p.active_at?)))
            .collect(),
        rows: measure(rows, at, readings),
        cpu_idle: match value("cpu_idle") {
            Some(idle) => idle
                .parse()
                .map_err(|_| format!("/proc/stat gave no idle share: {idle:?}")),
            None => parse_cpu(value("cpu").unwrap_or_default()),
        },
        jobs,
        sccache,
        display: crate::host_display::parse(head),
    };
    Ok((scan, cpu))
}

/// Runs the script on <host>, reads what it printed against the readings
/// kept in the caches folder under <home>, and keeps this listing as a reading
/// when the newest is a minute old (process_readings.rs).
pub fn read(ssh: &Ssh, host: &Host, home: &Path) -> std::result::Result<Scan, String> {
    let mut readings = process_readings::read(home, &host.name).map_err(|fault| fault.text())?;
    let text = crate::host::output_at(
        ssh,
        host,
        &script(host),
        &format!("reading {}", place(&host.name)),
    )?;
    let (scan, current) = read_scan(&text, &readings)?;
    if process_readings::record(&mut readings, current) {
        process_readings::write(home, &host.name, &readings).map_err(|fault| fault.text())?;
    }
    Ok(scan)
}

/// The rows of <rows> under <pid>, it included, nearest first.
pub fn tree(rows: &[Row], pid: i32) -> Vec<&Row> {
    trees(rows, &BTreeSet::from([pid]))
}

/// The roots and their descendants, each row once, nearest to the roots first.
pub fn trees<'a>(rows: &'a [Row], roots: &BTreeSet<i32>) -> Vec<&'a Row> {
    let mut children: BTreeMap<i32, Vec<&Row>> = BTreeMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row);
    }
    let mut found: Vec<&Row> = rows.iter().filter(|row| roots.contains(&row.pid)).collect();
    let mut seen = roots.clone();
    let mut at = 0;
    while at < found.len() {
        let parent = found[at].pid;
        found.extend(
            children
                .get(&parent)
                .into_iter()
                .flatten()
                .copied()
                .filter(|row| seen.insert(row.pid)),
        );
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
    crate::ack::agent::without_flag_of(row.command.split_whitespace())
        .into_iter()
        .skip(1)
        .collect()
}

#[cfg(test)]
#[path = "host_scan_tests.rs"]
mod tests;
