//! The record of every `builds app-build` run: each lane's run writes one row
//! to `builds.sqlite` in its repo's project store, with every step it started
//! (the schema is app_timing.sql). The dashboard reads the figures back; they
//! are SQL rows and Rust medians over the last `WINDOW_SECONDS`, never stored.
//!
//! A run records when it joined the lane's queue, how long it waited there
//! before it took a checkout (a wait after a takeover included), the checkout,
//! the ref and the claim of the worktree the caller ran in, and for each step
//! its start, wall time, CPU time and result. It is written once, when the run
//! ends, in one transaction. A write that fails is a warning and changes
//! nothing about the build's result, as with the hook's record.
//!
//! A step's figures count only its passing runs: the time of a failed,
//! timed-out or stopped step says nothing about how long the step takes. A
//! run's wait counts always.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::fail;
use crate::fail::{Context, Result};
use crate::hook_stats::median_of;
use crate::lane::Lane;

pub use crate::db::DATABASE;

/// The trailing time the dashboard's figures cover.
pub const WINDOW_SECONDS: i64 = crate::hook_stats::DASHBOARD_WINDOW_SECONDS;
/// How many of a step's latest runs the "last" median takes.
pub const LAST_RUNS: usize = 10;
/// How many of the longest waits the dashboard lists.
pub const LONGEST_WAITS: usize = 5;

/// What kind of step a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// A step of `.app-build`.
    Step,
    /// A `--test` class.
    Test,
    /// A `--shots` selection.
    Shots,
}

impl StepKind {
    pub fn word(self) -> &'static str {
        match self {
            StepKind::Step => "step",
            StepKind::Test => "test",
            StepKind::Shots => "shots",
        }
    }
}

/// How a step ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepResult {
    Passed,
    Failed,
    TimedOut,
    TakenOver,
}

impl StepResult {
    pub fn word(self) -> &'static str {
        match self {
            StepResult::Passed => "passed",
            StepResult::Failed => "failed",
            StepResult::TimedOut => "timed out",
            StepResult::TakenOver => "taken over",
        }
    }
}

/// One step a run started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepRecord {
    pub name: String,
    pub kind: StepKind,
    /// Seconds since 1970.
    pub started_at: i64,
    pub wall_ms: u64,
    pub cpu_ms: u64,
    pub result: StepResult,
}

/// One lane's run, as `record` writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRecord {
    /// When it joined the lane's queue, seconds since 1970.
    pub started_at: i64,
    pub repo: String,
    pub lane: Lane,
    pub checkout: String,
    pub built_ref: String,
    pub who: Option<String>,
    pub wait_ms: u64,
    pub passed: bool,
    pub steps: Vec<StepRecord>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The CPU time (user and system) of every child this process has waited for
/// so far.
pub fn children_cpu() -> Duration {
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage) };
    let part = |time: libc::timeval| {
        Duration::new(time.tv_sec.max(0) as u64, time.tv_usec.max(0) as u32 * 1000)
    };
    part(usage.ru_utime) + part(usage.ru_stime)
}

/// Times one step: start it with `Timer::start`, end it with `finish`.
pub struct Timer {
    started_at: i64,
    began: Instant,
    cpu: Duration,
}

impl Timer {
    pub fn start() -> Timer {
        Timer {
            started_at: now(),
            began: Instant::now(),
            cpu: children_cpu(),
        }
    }

    pub fn finish(self, name: &str, kind: StepKind, result: StepResult) -> StepRecord {
        StepRecord {
            name: name.to_string(),
            kind,
            started_at: self.started_at,
            wall_ms: self.began.elapsed().as_millis() as u64,
            cpu_ms: children_cpu().saturating_sub(self.cpu).as_millis() as u64,
            result,
        }
    }
}

/// The database of the project <repo> is in; None for a repo in no project,
/// which has nowhere to record.
pub fn database_of(
    places: &crate::places::Places,
    repo: &str,
) -> Result<Option<std::path::PathBuf>> {
    let mut found = crate::lists::store::projects_holding(places, repo)?;
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(places.lists.join(found.remove(0)).join(DATABASE))),
        _ => fail!(
            "repo {repo} is in two projects ({}); app-build runs have no one place to record",
            found.join(", ")
        ),
    }
}

/// Writes <run> and its steps to the existing database at <path>, in one
/// transaction.
pub fn record(path: &Path, run: &RunRecord) -> Result<()> {
    let mut conn = crate::db::open(path, crate::db::Make::Never)?;
    let what = || format!("writing {}", path.display());
    let tx = conn.transaction().context(what)?;
    tx.execute(
        "INSERT INTO app_build_runs (started_at, repo, lane, checkout, built_ref, who, wait_ms, passed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            run.started_at,
            run.repo,
            run.lane.word(),
            run.checkout,
            run.built_ref,
            run.who,
            run.wait_ms as i64,
            run.passed
        ],
    )
    .context(what)?;
    let id = tx.last_insert_rowid();
    {
        let mut step = tx
            .prepare("INSERT INTO app_build_steps (run_id, position, name, kind, started_at, wall_ms, cpu_ms, result) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
            .context(what)?;
        for (position, s) in run.steps.iter().enumerate() {
            step.execute(params![
                id,
                position as i64,
                s.name,
                s.kind.word(),
                s.started_at,
                s.wall_ms as i64,
                s.cpu_ms as i64,
                s.result.word()
            ])
            .context(what)?;
        }
    }
    tx.commit().context(what)
}

/// One step's figures in one repo and lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepStats {
    pub name: String,
    pub lane: String,
    pub kind: String,
    /// Passing runs in the window.
    pub ran: u32,
    pub median_ms: u64,
    /// The median of the latest `LAST_RUNS` passing runs.
    pub last_median_ms: u64,
    pub slowest_ms: u64,
    pub cpu_median_ms: u64,
}

/// A lane's queue wait over the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneWait {
    pub lane: String,
    pub runs: u32,
    pub median_ms: u64,
    pub slowest_ms: u64,
}

/// A run that waited long.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waited {
    pub at: i64,
    pub lane: String,
    pub checkout: String,
    pub built_ref: String,
    pub who: Option<String>,
    pub wait_ms: u64,
    pub passed: bool,
}

/// One repo's figures over the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoStats {
    pub repo: String,
    pub runs: u32,
    /// By lane, then by median time, slowest first.
    pub steps: Vec<StepStats>,
    pub lanes: Vec<LaneWait>,
    /// The runs in the window that waited longest, longest first.
    pub longest_waits: Vec<Waited>,
}

/// The figures of every repo with a run since <since> (seconds since 1970) in
/// the database at <path>, by repo name; empty when nothing was recorded.
pub fn stats(path: &Path, since: i64) -> Result<Vec<RepoStats>> {
    let Some(conn) = crate::db::open_reader(path)? else {
        return Ok(Vec::new());
    };
    let what = || format!("reading {}", path.display());
    let mut repos: Vec<String> = Vec::new();
    {
        let mut statement = conn
            .prepare(
                "SELECT DISTINCT repo FROM app_build_runs WHERE started_at >= ?1 ORDER BY repo",
            )
            .context(what)?;
        for row in statement
            .query_map([since], |row| row.get::<_, String>(0))
            .context(what)?
        {
            repos.push(row.context(what)?);
        }
    }
    repos
        .into_iter()
        .map(|repo| repo_stats(&conn, &repo, since, path))
        .collect()
}

/// A step's lane, name and kind.
type StepKey = (String, String, String);

fn repo_stats(conn: &Connection, repo: &str, since: i64, path: &Path) -> Result<RepoStats> {
    let what = || format!("reading {}", path.display());
    let mut statement = conn
        .prepare(
            "SELECT r.lane, s.name, s.kind, s.wall_ms, s.cpu_ms
             FROM app_build_steps s JOIN app_build_runs r ON r.id = s.run_id
             WHERE r.repo = ?1 AND s.started_at >= ?2 AND s.result = 'passed'
             ORDER BY s.started_at DESC, s.run_id DESC, s.position DESC",
        )
        .context(what)?;
    let rows = statement
        .query_map(params![repo, since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, i64>(4)? as u64,
            ))
        })
        .context(what)?;
    // Newest first, so each group's first LAST_RUNS are its latest.
    let mut groups: Vec<(StepKey, Vec<(u64, u64)>)> = Vec::new();
    for row in rows {
        let (lane, name, kind, wall, cpu) = row.context(what)?;
        let key = (lane, name, kind);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, times)) => times.push((wall, cpu)),
            None => groups.push((key, vec![(wall, cpu)])),
        }
    }
    let mut steps: Vec<StepStats> = groups
        .into_iter()
        .map(|((lane, name, kind), times)| {
            let walls: Vec<u64> = times.iter().map(|(wall, _)| *wall).collect();
            StepStats {
                name,
                lane,
                kind,
                ran: times.len() as u32,
                median_ms: median_of(&mut walls.clone()),
                last_median_ms: median_of(&mut walls[..walls.len().min(LAST_RUNS)].to_vec()),
                slowest_ms: walls.iter().copied().max().unwrap_or(0),
                cpu_median_ms: median_of(
                    &mut times.iter().map(|(_, cpu)| *cpu).collect::<Vec<_>>(),
                ),
            }
        })
        .collect();
    steps.sort_by(|a, b| {
        a.lane
            .cmp(&b.lane)
            .then(b.median_ms.cmp(&a.median_ms))
            .then(a.name.cmp(&b.name))
    });

    let mut statement = conn
        .prepare(
            "SELECT started_at, lane, checkout, built_ref, who, wait_ms, passed
             FROM app_build_runs WHERE repo = ?1 AND started_at >= ?2
             ORDER BY wait_ms DESC, started_at DESC, id DESC",
        )
        .context(what)?;
    let waited: Vec<Waited> = statement
        .query_map(params![repo, since], |row| {
            Ok(Waited {
                at: row.get(0)?,
                lane: row.get(1)?,
                checkout: row.get(2)?,
                built_ref: row.get(3)?,
                who: row.get(4)?,
                wait_ms: row.get::<_, i64>(5)? as u64,
                passed: row.get(6)?,
            })
        })
        .context(what)?
        .collect::<std::result::Result<_, _>>()
        .context(what)?;
    let mut lanes: Vec<LaneWait> = Vec::new();
    for lane in Lane::ALL {
        let mut times: Vec<u64> = waited
            .iter()
            .filter(|w| w.lane == lane.word())
            .map(|w| w.wait_ms)
            .collect();
        if !times.is_empty() {
            lanes.push(LaneWait {
                lane: lane.word().to_string(),
                runs: times.len() as u32,
                slowest_ms: times.iter().copied().max().unwrap_or(0),
                median_ms: median_of(&mut times),
            });
        }
    }
    Ok(RepoStats {
        repo: repo.to_string(),
        runs: waited.len() as u32,
        steps,
        lanes,
        longest_waits: waited.into_iter().take(LONGEST_WAITS).collect(),
    })
}

/// The dashboard's JSON for the project store at <path>, over the
/// `WINDOW_SECONDS` before <now>: `{"window_seconds", "repos": [...]}`, or
/// `{"error"}` when the database can't be read.
pub fn dashboard_json(path: &Path, now: i64) -> serde_json::Value {
    match stats(path, now - WINDOW_SECONDS) {
        Err(error) => serde_json::json!({ "error": error.text() }),
        Ok(repos) => serde_json::json!({
            "window_seconds": WINDOW_SECONDS,
            "last_runs": LAST_RUNS,
            "repos": repos.iter().map(|repo| serde_json::json!({
                "repo": repo.repo,
                "runs": repo.runs,
                "steps": repo.steps.iter().map(|s| serde_json::json!({
                    "name": s.name, "lane": s.lane, "kind": s.kind, "ran": s.ran,
                    "median_ms": s.median_ms, "last_median_ms": s.last_median_ms,
                    "slowest_ms": s.slowest_ms, "cpu_median_ms": s.cpu_median_ms,
                })).collect::<Vec<_>>(),
                "lanes": repo.lanes.iter().map(|l| serde_json::json!({
                    "lane": l.lane, "runs": l.runs, "median_ms": l.median_ms, "slowest_ms": l.slowest_ms,
                })).collect::<Vec<_>>(),
                "longest_waits": repo.longest_waits.iter().map(|w| serde_json::json!({
                    "at": w.at, "lane": w.lane, "checkout": w.checkout, "ref": w.built_ref,
                    "who": w.who, "wait_ms": w.wait_ms, "passed": w.passed,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }),
    }
}

#[cfg(test)]
#[path = "app_timing_tests.rs"]
mod tests;
