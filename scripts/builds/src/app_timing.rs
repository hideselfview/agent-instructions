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

use crate::app_config::PickKind;
use crate::fail;
use crate::fail::{Context, Result};
use crate::hook_stats::median_of;
use crate::lane::Lane;

pub use crate::db::DATABASE;

/// The trailing time the dashboard's figures cover.
pub const WINDOW_SECONDS: i64 = crate::hook_stats::DASHBOARD_WINDOW_SECONDS;
/// How many of a lane's latest runs the wait figure draws a bar for.
pub const WAITS_SHOWN: usize = 20;

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
    /// The kind `word` names, as the store holds it (its CHECK allows no other).
    pub fn from_word(word: &str) -> StepKind {
        match word {
            "test" => StepKind::Test,
            "shots" => StepKind::Shots,
            _ => StepKind::Step,
        }
    }

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
#[derive(Clone, Debug, PartialEq)]
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
    /// The cores and load of the checkout's host when the run took the
    /// checkout; None when the host did not answer.
    pub load: Option<crate::host::Load>,
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
        "INSERT INTO app_build_runs (started_at, repo, lane, checkout, built_ref, who, wait_ms, passed, cores, load) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            run.started_at,
            run.repo,
            run.lane.word(),
            run.checkout,
            run.built_ref,
            run.who,
            run.wait_ms as i64,
            run.passed,
            run.load.map(|load| load.cores),
            run.load.map(|load| load.one_minute)
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

/// Makes `app_build_runs` again as app_timing.sql writes it today, with the
/// load of each run's host, and copies every run into it with no load: no
/// run recorded before has one. The statements come from app_timing.sql, so
/// the table equals a fresh database's. migrate.rs runs this with the foreign
/// keys off, so `app_build_steps` keeps naming the table.
pub fn move_to_loads(db: &Connection) -> Result<()> {
    use crate::db::Sql;
    crate::db::remake_table(
        db,
        "app_build_runs",
        crate::db::statement(
            crate::db::APP_TIMING_PART,
            "CREATE TABLE IF NOT EXISTS app_build_runs",
            "STRICT;",
        ),
        |db| {
            db.execute_batch(
                "INSERT INTO app_build_runs (id, started_at, repo, lane, checkout, built_ref, who, wait_ms, passed)
    SELECT id, started_at, repo, lane, checkout, built_ref, who, wait_ms, passed FROM app_build_runs_old;",
            )
            .sql()
        },
        &[crate::db::statement(
            crate::db::APP_TIMING_PART,
            "CREATE INDEX IF NOT EXISTS idx_app_build_runs_started",
            ";",
        )],
    )
}

/// One step's figures in one repo and lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepStats {
    /// As the dashboard words it: `macos`, `Foo` for the test class `Foo`,
    /// `mac.welcome.*` for the shot prefix `mac.welcome.`.
    pub name: String,
    pub kind: StepKind,
    /// Passing runs in the window.
    pub ran: u32,
    pub median_ms: u64,
    pub slowest_ms: u64,
    /// The time the step took in all: its runs times its median.
    pub total_ms: u64,
}

/// One app build lane's figures in one repo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneStats {
    pub lane: Lane,
    /// Runs in the window, whether they passed or not.
    pub runs: u32,
    /// The median time a run waited in the lane's queue.
    pub median_wait_ms: u64,
    /// The latest `WAITS_SHOWN` runs' waits, oldest first.
    pub waits_ms: Vec<u64>,
    /// The steps that passed, the one that took the most time in all first.
    pub steps: Vec<StepStats>,
}

/// One repo's figures over the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoStats {
    pub repo: String,
    /// The lanes with a run in the window, mac first.
    pub lanes: Vec<LaneStats>,
}

/// How the dashboard words a step recorded under <name>.
fn step_word(name: &str, kind: StepKind) -> String {
    let (prefix, star) = match kind {
        StepKind::Step => return name.to_string(),
        StepKind::Test => (PickKind::Class.step_prefix(), ""),
        StepKind::Shots => (PickKind::Shots.step_prefix(), "*"),
    };
    let bare = name.strip_prefix(prefix).unwrap_or(name);
    if star.is_empty() || !bare.ends_with('.') {
        bare.to_string()
    } else {
        format!("{bare}{star}")
    }
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

fn repo_stats(conn: &Connection, repo: &str, since: i64, path: &Path) -> Result<RepoStats> {
    let what = || format!("reading {}", path.display());
    let mut lanes = Vec::new();
    for lane in Lane::ALL {
        // Oldest first.
        let mut statement = conn
            .prepare(
                "SELECT wait_ms FROM app_build_runs
                 WHERE repo = ?1 AND lane = ?2 AND started_at >= ?3
                 ORDER BY started_at, id",
            )
            .context(what)?;
        let waits: Vec<u64> = statement
            .query_map(params![repo, lane.word(), since], |row| {
                Ok(row.get::<_, i64>(0)? as u64)
            })
            .context(what)?
            .collect::<std::result::Result<_, _>>()
            .context(what)?;
        if waits.is_empty() {
            continue;
        }
        // Each step's passing wall times, grouped by step.
        let mut statement = conn
            .prepare(
                "SELECT s.name, s.kind, s.wall_ms
                 FROM app_build_steps s JOIN app_build_runs r ON r.id = s.run_id
                 WHERE r.repo = ?1 AND r.lane = ?2 AND s.started_at >= ?3 AND s.result = 'passed'
                 ORDER BY s.name, s.kind",
            )
            .context(what)?;
        let rows = statement
            .query_map(params![repo, lane.word(), since], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                ))
            })
            .context(what)?;
        let mut groups: Vec<(String, String, Vec<u64>)> = Vec::new();
        for row in rows {
            let (name, kind, wall) = row.context(what)?;
            match groups.last_mut() {
                Some((n, k, times)) if *n == name && *k == kind => times.push(wall),
                _ => groups.push((name, kind, vec![wall])),
            }
        }
        let mut steps: Vec<StepStats> = groups
            .into_iter()
            .map(|(name, kind, mut times)| {
                let kind = StepKind::from_word(&kind);
                let ran = times.len() as u32;
                let slowest_ms = times.iter().copied().max().unwrap_or(0);
                let median_ms = median_of(&mut times);
                StepStats {
                    name: step_word(&name, kind),
                    kind,
                    ran,
                    median_ms,
                    slowest_ms,
                    total_ms: u64::from(ran) * median_ms,
                }
            })
            .collect();
        steps.sort_by(|a, b| b.total_ms.cmp(&a.total_ms).then(a.name.cmp(&b.name)));
        let runs = waits.len() as u32;
        let mut sorted = waits.clone();
        lanes.push(LaneStats {
            lane,
            runs,
            median_wait_ms: median_of(&mut sorted),
            waits_ms: waits[waits.len().saturating_sub(WAITS_SHOWN)..].to_vec(),
            steps,
        });
    }
    Ok(RepoStats {
        repo: repo.to_string(),
        lanes,
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
            "repos": repos.iter().map(|repo| serde_json::json!({
                "repo": repo.repo,
                "lanes": repo.lanes.iter().map(|l| serde_json::json!({
                    "lane": l.lane.word(),
                    "runs": l.runs,
                    "median_wait_ms": l.median_wait_ms,
                    "waits_ms": l.waits_ms,
                    "steps": l.steps.iter().map(|s| serde_json::json!({
                        "name": s.name, "kind": s.kind.word(), "ran": s.ran,
                        "median_ms": s.median_ms, "slowest_ms": s.slowest_ms,
                        "total_ms": s.total_ms,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }),
    }
}

/// Writes a row of one of app_timing.rs's tables as given, the constraints
/// still holding: for tests that need a row of a store they build by hand.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &["app_build_runs", "app_build_steps"], table, columns)
}

#[cfg(test)]
#[path = "app_timing_tests.rs"]
mod tests;
