//! The record of every `builds app-build` run: each lane's run writes one row
//! to `builds.sqlite` in its repo's project store, with every step it started
//! (the schema is app_timing.sql). The dashboard reads the figures back; they
//! are SQL rows and Rust medians over the last `WINDOW_SECONDS`, never stored.
//!
//! A run records when it joined the lane's queue, how long it waited there
//! before it took a checkout (a wait after a takeover included), the checkout,
//! the ref and the claim of the worktree the caller ran in, how it ended
//! (passed, failed, or stopped by a stop signal), and for each step its start,
//! wall time, CPU time and result. It is written once, when the run ends, in
//! one transaction. A write that fails is a warning and changes
//! nothing about the build's result, as with the hook's record.
//!
//! A step's figures count only its passing runs: the time of a failed,
//! timed-out or stopped step says nothing about how long the step takes. A
//! run's wait counts always.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::app_config::PickKind;
use crate::clock::Moment;
use crate::fail;
use crate::fail::{Context, Result};
use crate::hook_stats::median_of;
use crate::lane::Lane;
use crate::run_end::RunEnd;
use crate::step_failure::StepFailure;

use crate::branch_end::BranchEnd;
pub use crate::db::DATABASE;
use crate::db::Sql;

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
    /// The kind `word` names, as the store holds it, or None for a word that
    /// is no kind's.
    pub fn from_word(word: &str) -> Option<StepKind> {
        [StepKind::Step, StepKind::Test, StepKind::Shots]
            .into_iter()
            .find(|kind| kind.word() == word)
    }

    pub fn word(self) -> &'static str {
        match self {
            StepKind::Step => "step",
            StepKind::Test => "test",
            StepKind::Shots => "shots",
        }
    }
}

/// How a step ended. A step that failed or ran past its limit carries what
/// it said (app_build_step_failure.rs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepResult {
    Passed,
    Failed(StepFailure),
    TimedOut(StepFailure),
    TakenOver,
    /// A stop signal (SIGTERM, SIGINT or SIGHUP) ended it.
    Stopped,
}

impl StepResult {
    pub fn word(&self) -> &'static str {
        match self {
            StepResult::Passed => "passed",
            StepResult::Failed(_) => "failed",
            StepResult::TimedOut(_) => "timed out",
            StepResult::TakenOver => "taken over",
            StepResult::Stopped => "stopped",
        }
    }

    /// The result `word` names, as the store holds it. A step that failed or
    /// ran past its limit carries <failure>, what the store kept of what it
    /// said; a store with none for such a step is damaged.
    pub fn from_word(word: &str, failure: Option<StepFailure>) -> Result<StepResult> {
        let said = || match failure.clone() {
            Some(failure) => Ok(failure),
            None => fail!("a step that {word} has no failure row"),
        };
        Ok(match word {
            "passed" => StepResult::Passed,
            "failed" => StepResult::Failed(said()?),
            "timed out" => StepResult::TimedOut(said()?),
            "taken over" => StepResult::TakenOver,
            "stopped" => StepResult::Stopped,
            other => fail!("a step ended {other:?}"),
        })
    }

    /// What a step that failed or ran past its limit said.
    pub fn failure(&self) -> Option<&StepFailure> {
        match self {
            StepResult::Failed(failure) | StepResult::TimedOut(failure) => Some(failure),
            StepResult::Passed | StepResult::TakenOver | StepResult::Stopped => None,
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
    /// How long the step took, the time the Mac slept during it included.
    pub wall_ms: u64,
    pub cpu_ms: u64,
    /// Whether the step ran at the merge base to make the run the commit's
    /// shots compare with (true) or at the commit (false); None for a step
    /// that builds recorded before it kept this, and until the lane's run of
    /// the commit sets it.
    pub at_merge_base: Option<bool>,
    pub result: StepResult,
    /// What a shots step used (app_build_step_shots.rs); None for any other
    /// step.
    pub shots: Option<crate::app_build_step_shots::ShotsUse>,
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
    /// The commit the requested ref resolved to. Older records lack a target.
    pub requested_commit: Option<String>,
    /// The commit the run checked out for <built_ref>; None when the run
    /// ended before it checked one out.
    pub built_commit: Option<String>,
    pub who: Option<String>,
    pub wait_ms: u64,
    pub outcome: RunEnd,
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
    /// When the step began, on a clock that counts the time the machine
    /// sleeps during it (clock.rs).
    began: Moment,
    cpu: Duration,
}

impl Timer {
    pub fn start() -> Timer {
        Timer {
            started_at: now(),
            began: Moment::now(),
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
            at_merge_base: None,
            result,
            shots: None,
        }
    }
}

/// The database of the project <repo> is in; None for a repo in no project,
/// which has nowhere to record.
pub fn database_of(
    places: &crate::places::Places,
    repo: &str,
) -> Result<Option<std::path::PathBuf>> {
    let mut found = crate::lists::store::databases_holding(places, repo)?;
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found.remove(0))),
        _ => fail!(
            "repo {repo} is in two projects; app-build runs have no one place to record ({})",
            found
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Writes <run>, its steps and the commit it built (app_build_commit.rs) to
/// the existing database at <path>, in one transaction.
pub fn record(path: &Path, run: &RunRecord) -> Result<()> {
    let mut conn = crate::db::open(path, crate::db::Make::Never)?;
    let what = || format!("writing {}", path.display());
    let tx = conn.transaction().context(what)?;
    tx.execute(
        "INSERT INTO app_build_runs (started_at, repo, lane, checkout, built_ref, who, wait_ms, outcome, cores, load) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            run.started_at,
            run.repo,
            run.lane.word(),
            run.checkout,
            run.built_ref,
            run.who,
            run.wait_ms as i64,
            run.outcome.word(),
            run.load.map(|load| load.cores),
            run.load.map(|load| load.one_minute)
        ],
    )
    .context(what)?;
    let id = tx.last_insert_rowid();
    {
        let mut step = tx
            .prepare("INSERT INTO app_build_steps (run_id, position, name, kind, started_at, wall_ms, cpu_ms, at_merge_base, result) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")
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
                s.at_merge_base,
                s.result.word()
            ])
            .context(what)?;
        }
    }
    crate::app_build_step_failure::record(&tx, id, &run.steps)?;
    crate::app_build_step_shots::record(&tx, id, &run.steps)?;
    if let Some(commit) = &run.requested_commit {
        crate::app_build_commit::record_requested(&tx, id, commit).context(what)?;
    }
    if let Some(commit) = &run.built_commit {
        crate::app_build_commit::record(&tx, id, commit).context(what)?;
    }
    tx.commit().context(what)
}

/// A step's columns as `read_run` reads them: name, kind, start, wall time,
/// CPU time, whether it ran at the merge base, and result.
type StepRow = (String, String, i64, i64, i64, Option<bool>, String);

/// The one run the database at <path> records, as `record` wrote it: what a
/// lane's run hands the installed builds when an install moved the stores
/// while the run built (handed_runs.rs). Fails when the store holds none or
/// more than one.
pub fn read_run(path: &Path) -> Result<RunRecord> {
    let Some(conn) = crate::db::open_reader(path)? else {
        fail!("{} does not exist", path.display());
    };
    let what = || format!("reading {}", path.display());
    let row = |row: &rusqlite::Row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, Option<i64>>(10)?,
            row.get::<_, Option<f64>>(11)?,
            row.get::<_, Option<String>>(12)?,
        ))
    };
    let mut found = conn
        .prepare(
            "SELECT r.id, r.started_at, r.repo, r.lane, r.checkout, r.built_ref, c.built_commit, r.who, r.wait_ms, r.outcome, r.cores, r.load, q.requested_commit
             FROM app_build_runs r LEFT JOIN app_build_run_commit c ON c.run_id = r.id
             LEFT JOIN app_build_requested_commit q ON q.run_id = r.id
             ORDER BY r.id",
        )
        .context(what)?
        .query_map([], row)
        .context(what)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context(what)?;
    let (
        id,
        started_at,
        repo,
        lane,
        checkout,
        built_ref,
        built_commit,
        who,
        wait_ms,
        outcome,
        cores,
        load,
        requested_commit,
    ) = match (found.pop(), found.is_empty()) {
        (Some(one), true) => one,
        _ => fail!("{} does not hold exactly one run", path.display()),
    };
    let mut run = RunRecord {
        started_at,
        repo,
        lane: Lane::parse_or_fail(&lane)?,
        checkout,
        built_ref,
        requested_commit,
        built_commit,
        who,
        wait_ms: wait_ms as u64,
        outcome: match RunEnd::from_word(&outcome) {
            Some(outcome) => outcome,
            None => fail!("{}: run {id} ended {outcome:?}", path.display()),
        },
        load: cores
            .zip(load)
            .map(|(cores, one_minute)| crate::host::Load {
                cores: cores as u32,
                one_minute,
            }),
        steps: Vec::new(),
    };
    let mut said = crate::app_build_step_failure::read(&conn, id)?;
    let mut used = crate::app_build_step_shots::read(&conn, id)?;
    let steps: Vec<StepRow> = conn
        .prepare(
            "SELECT name, kind, started_at, wall_ms, cpu_ms, at_merge_base, result FROM app_build_steps
             WHERE run_id = ?1 ORDER BY position",
        )
        .context(what)?
        .query_map([id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .context(what)?
        .collect::<std::result::Result<_, _>>()
        .context(what)?;
    for (position, (name, kind, started_at, wall_ms, cpu_ms, at_merge_base, result)) in
        steps.into_iter().enumerate()
    {
        let Some(kind) = StepKind::from_word(&kind) else {
            fail!(
                "{}: step {name} of run {id} is of kind {kind:?}",
                path.display()
            );
        };
        let result =
            StepResult::from_word(&result, said.remove(&(position as u32))).map_err(|error| {
                crate::fail::Fail::Message(format!(
                    "{}: step {name} of run {id}: {}",
                    path.display(),
                    error.text()
                ))
            })?;
        run.steps.push(StepRecord {
            name,
            kind,
            started_at,
            wall_ms: wall_ms as u64,
            cpu_ms: cpu_ms as u64,
            at_merge_base,
            result,
            shots: used.remove(&(position as u32)),
        });
    }
    Ok(run)
}

/// Moves a store from before an app build run could end stopped: makes the
/// runs' table again with how each run ended in place of whether it passed
/// (a run that passed keeps `passed`, and every other run is `failed`), and
/// the steps' table again with the check that takes a stopped step. Each run
/// and step keeps its row. migrate.rs runs it with the foreign keys off.
pub fn record_stopped_runs(db: &Connection) -> Result<()> {
    use crate::db::{Sql, remake_table, statement};
    let schema = include_str!("app_timing.sql");
    remake_table(
        db,
        "app_build_runs",
        // A comment in the table holds a semicolon, so the statement ends at
        // the table's options.
        statement(
            schema,
            "CREATE TABLE IF NOT EXISTS app_build_runs",
            ") STRICT;",
        ),
        |db| {
            db.execute_batch(
                "INSERT INTO app_build_runs (id, started_at, repo, lane, checkout, built_ref, who, wait_ms, outcome, cores, load) SELECT id, started_at, repo, lane, checkout, built_ref, who, wait_ms, CASE passed WHEN 1 THEN 'passed' ELSE 'failed' END, cores, load FROM app_build_runs_old",
            )
            .sql()
        },
        &[statement(
            schema,
            "CREATE INDEX IF NOT EXISTS idx_app_build_runs_started",
            ";",
        )],
    )?;
    remake_table(
        db,
        "app_build_steps",
        statement(
            schema,
            "CREATE TABLE IF NOT EXISTS app_build_steps",
            ") STRICT, WITHOUT ROWID;",
        ),
        |db| {
            db.execute_batch(
                "INSERT INTO app_build_steps (run_id, position, name, kind, started_at, wall_ms, cpu_ms, result) SELECT run_id, position, name, kind, started_at, wall_ms, cpu_ms, result FROM app_build_steps_old",
            )
            .sql()
        },
        &[statement(
            schema,
            "CREATE INDEX IF NOT EXISTS idx_app_build_steps_name",
            ";",
        )],
    )
}

/// Makes `app_build_steps` again as app_timing.sql writes it, with whether
/// each step ran at the merge base: a store from before kept no such column,
/// and each step in it has NULL, since builds never recorded which it was.
pub fn add_at_merge_base(db: &Connection) -> Result<()> {
    use crate::db::{Sql, remake_table, statement};
    let schema = include_str!("app_timing.sql");
    remake_table(
        db,
        "app_build_steps",
        statement(
            schema,
            "CREATE TABLE IF NOT EXISTS app_build_steps",
            ") STRICT, WITHOUT ROWID;",
        ),
        |db| {
            db.execute_batch(
                "INSERT INTO app_build_steps (run_id, position, name, kind, started_at, wall_ms, cpu_ms, result) SELECT run_id, position, name, kind, started_at, wall_ms, cpu_ms, result FROM app_build_steps_old",
            )
            .sql()
        },
        &[statement(
            schema,
            "CREATE INDEX IF NOT EXISTS idx_app_build_steps_name",
            ";",
        )],
    )
}

/// A passing app build run: the commit it built (app_build_commit.rs) and
/// the steps that passed in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassedRun {
    pub commit: String,
    pub steps: Vec<String>,
}

impl PassedRun {
    /// The prefixes of the shots steps that passed in the run.
    pub fn shots_prefixes(&self) -> impl Iterator<Item = &str> {
        self.steps
            .iter()
            .filter_map(|step| PickKind::Shots.argument_of_step(step))
    }

    /// Whether a shots step of the run covers <prefix>: a run of the prefix,
    /// or of a prefix it starts with (`mac.` runs every shot of the lane).
    pub fn ran_shots(&self, prefix: &str) -> bool {
        self.shots_prefixes().any(|ran| prefix.starts_with(ran))
    }
}

/// A run belongs to a branch by its requested ref or its commit. The query
/// supplies the commit column or expression it reads.
fn branch_run_match(commit: &str) -> String {
    format!("(r.built_ref IS ?3 OR {commit} IN (SELECT value FROM json_each(?4)))")
}

/// The passing runs of <repo>'s <lane> that recorded the commit they built
/// and are a branch's: the ref the caller named (`built_ref`) is <named>, when
/// there is one, or the commit they built is one of <commits>, the branch's
/// commits since its merge base. The database is open as <conn> at <path>. Newest first, however
/// many other runs came after them.
pub fn passed_runs(
    conn: &Connection,
    repo: &str,
    lane: Lane,
    path: &Path,
    named: Option<&str>,
    commits: &[String],
) -> Result<Vec<PassedRun>> {
    let what = || format!("reading {}", path.display());
    let commits = serde_json::to_string(commits).context(what)?;
    let matches_branch = branch_run_match("c.built_commit");
    let mut runs = conn
        .prepare(
            format!(
                "SELECT r.id, c.built_commit FROM app_build_runs r
                 JOIN app_build_run_commit c ON c.run_id = r.id
                 WHERE r.repo = ?1 AND r.lane = ?2 AND r.outcome = 'passed'
                   AND {matches_branch}
                 ORDER BY r.id DESC"
            )
            .as_str(),
        )
        .context(what)?;
    let found: Vec<(i64, String)> = runs
        .query_map(params![repo, lane.word(), named, commits], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .context(what)?
        .collect::<std::result::Result<_, _>>()
        .context(what)?;
    found
        .into_iter()
        .map(|(id, commit)| {
            Ok(PassedRun {
                commit,
                steps: passed_steps(conn, id, path)?,
            })
        })
        .collect()
}

/// The names of the steps that passed in the run <id>, in the order they
/// ran.
fn passed_steps(conn: &Connection, id: i64, path: &Path) -> Result<Vec<String>> {
    let what = || format!("reading {}", path.display());
    conn.prepare(
        "SELECT name FROM app_build_steps WHERE run_id = ?1 AND result = 'passed' ORDER BY position",
    )
    .context(what)?
    .query_map([id], |row| row.get(0))
    .context(what)?
    .collect::<std::result::Result<_, _>>()
    .context(what)
}

/// Whether a passing app build of <repo>'s <lane> built <commit> and passed
/// the shots run of <prefix>, in the database open as <conn> at <path>.
pub fn passed_shots(
    conn: &Connection,
    repo: &str,
    lane: Lane,
    commit: &str,
    prefix: &str,
    path: &Path,
) -> Result<bool> {
    let own = [commit.to_string()];
    Ok(passed_runs(conn, repo, lane, path, None, &own)?
        .iter()
        .any(|run| run.ran_shots(prefix)))
}

/// The shots prefixes that app builds of <repo>'s <lane> ran to a verdict
/// when asked for <named> or targeting one of <commits>. The target matches
/// even if the run ended before reaching its checkout.
/// Counts runs that started at or after <since> (seconds since 1970),
/// whatever their outcome, in the database open as <conn> at <path>.
/// Returns each prefix once, in name order.
pub fn shots_ran(
    conn: &Connection,
    repo: &str,
    lane: Lane,
    named: &str,
    commits: &[String],
    since: i64,
    path: &Path,
) -> Result<Vec<String>> {
    let what = || format!("reading {}", path.display());
    let commits = serde_json::to_string(commits).context(what)?;
    let matches_branch = branch_run_match("COALESCE(c.built_commit, q.requested_commit)");
    let mut statement = conn
        .prepare(
            format!(
                "SELECT DISTINCT s.name FROM app_build_steps s
                 JOIN app_build_runs r ON r.id = s.run_id
                 LEFT JOIN app_build_run_commit c ON c.run_id = r.id
                 LEFT JOIN app_build_requested_commit q ON q.run_id = r.id
                 WHERE r.repo = ?1 AND r.lane = ?2 AND r.started_at >= ?5
                   AND {matches_branch} AND {SHOTS_VERDICT}
                 ORDER BY s.name"
            )
            .as_str(),
        )
        .context(what)?;
    let names: Vec<String> = statement
        .query_map(params![repo, lane.word(), named, commits, since], |row| {
            row.get(0)
        })
        .context(what)?
        .collect::<std::result::Result<_, _>>()
        .context(what)?;
    names
        .into_iter()
        .map(|name| Ok(shots_prefix(&name, path)?.to_string()))
        .collect()
}

/// What the SQL of a query over shots steps needs to keep: steps that ran to
/// a verdict.
const SHOTS_VERDICT: &str = "s.kind = 'shots' AND s.result IN ('passed', 'failed', 'timed out')";

/// The shots prefix a shots step <name> names, in the database at <path>.
fn shots_prefix<'a>(name: &'a str, path: &Path) -> Result<&'a str> {
    let kind = PickKind::Shots;
    match kind.argument_of_step(name) {
        Some(prefix) => Ok(prefix),
        None => fail!(
            "{} holds a shots step named {name:?}, which does not start with {:?}",
            path.display(),
            kind.step_prefix()
        ),
    }
}

/// One shots run that an app build made of a commit: the branch the build
/// named, the commit it built, and the shots prefix it ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotsBuild {
    pub branch: String,
    pub commit: String,
    pub lane: Lane,
    pub prefix: String,
    /// When the app build joined the lane's queue, seconds since 1970.
    pub started_at: i64,
}

/// The shots runs that app builds of <repo> made at or after <since>
/// (seconds since 1970) of the commit they built, to a verdict, passed or
/// not, in the database open as <conn> at <path>. A build that recorded no
/// commit has none.
pub fn shots_builds(
    conn: &Connection,
    repo: &str,
    since: i64,
    path: &Path,
) -> Result<Vec<ShotsBuild>> {
    let what = || format!("reading {}", path.display());
    let mut statement = conn
        .prepare(
            format!(
                "SELECT DISTINCT r.built_ref, c.built_commit, r.lane, s.name, r.started_at
                 FROM app_build_steps s
                 JOIN app_build_runs r ON r.id = s.run_id
                 JOIN app_build_run_commit c ON c.run_id = r.id
                 WHERE r.repo = ?1 AND r.started_at >= ?2 AND {SHOTS_VERDICT}
                 ORDER BY r.started_at, s.name"
            )
            .as_str(),
        )
        .context(what)?;
    let rows: Vec<(String, String, String, String, i64)> = statement
        .query_map(params![repo, since], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .context(what)?
        .collect::<std::result::Result<_, _>>()
        .context(what)?;
    rows.into_iter()
        .map(|(branch, commit, lane, name, started_at)| {
            Ok(ShotsBuild {
                branch,
                commit,
                lane: Lane::parse_or_fail(&lane)?,
                prefix: shots_prefix(&name, path)?.to_string(),
                started_at,
            })
        })
        .collect()
}

/// Each app build run's queue wait and each step, of the runs and steps that
/// started at or after <since>, as waits (waits.rs), whatever their result,
/// each with the load of the run's host when the run took its checkout.
pub fn waits(conn: &Connection, since: i64, path: &Path) -> Result<Vec<crate::waits::Wait>> {
    use crate::waits::{Kind, Wait};
    let what = || format!("reading {}", path.display());
    let mut waits = Vec::new();
    for (kind, sql) in [
        (
            Kind::AppBuildQueue,
            "SELECT started_at, wait_ms, cores, load FROM app_build_runs WHERE started_at >= ?1",
        ),
        (
            Kind::AppBuildStep,
            "SELECT s.started_at, s.wall_ms, r.cores, r.load
             FROM app_build_steps s JOIN app_build_runs r ON r.id = s.run_id
             WHERE s.started_at >= ?1",
        ),
    ] {
        let mut statement = conn.prepare(sql).context(what)?;
        let rows = statement
            .query_map([since], |row| {
                Ok(Wait {
                    kind,
                    started_at: row.get(0)?,
                    ms: row.get::<_, i64>(1)? as u64,
                    load: crate::host::Load::from_columns(row.get(2)?, row.get(3)?),
                })
            })
            .context(what)?;
        for row in rows {
            waits.push(row.context(what)?);
        }
    }
    Ok(waits)
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
    let (pick, star) = match kind {
        StepKind::Step => return name.to_string(),
        StepKind::Test => (PickKind::Class, ""),
        StepKind::Shots => (PickKind::Shots, "*"),
    };
    let bare = pick.argument_of_step(name).unwrap_or(name);
    if star.is_empty() || !bare.ends_with('.') {
        bare.to_string()
    } else {
        format!("{bare}{star}")
    }
}

/// For each checkout that ran a <lane> build, the time its latest run in the
/// database at <path> ended: the end of its last step, or the end of its wait
/// when it ran none (seconds since 1970). The records outlive a checkout that
/// builds removed. Empty when nothing was recorded.
pub fn last_ends(path: &Path, lane: Lane) -> Result<Vec<(String, i64)>> {
    let Some(conn) = crate::db::open_reader(path)? else {
        return Ok(Vec::new());
    };
    let what = || format!("reading {}", path.display());
    let mut statement = conn
        .prepare(&format!(
            "SELECT r.checkout, MAX({RUN_END}) FROM app_build_runs r WHERE r.lane = ?1 GROUP BY r.checkout"
        ))
        .context(what)?;
    let rows = statement
        .query_map([lane.word()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .context(what)?;
    rows.map(|row| row.context(what)).collect()
}

const RUN_END: &str = "COALESCE(
    (SELECT MAX(s.started_at + s.wall_ms / 1000) FROM app_build_steps s WHERE s.run_id = r.id),
    r.started_at + r.wait_ms / 1000)";

pub fn branch_ends(db: &Connection) -> Result<Vec<BranchEnd>> {
    let mut statement = db
        .prepare(&format!(
            "SELECT r.repo, r.built_ref, MAX({RUN_END}) FROM app_build_runs r
         GROUP BY r.repo, r.built_ref ORDER BY r.repo, r.built_ref"
        ))
        .sql()?;
    let rows = statement.query_map([], BranchEnd::from_row).sql()?;
    rows.map(|row| row.sql()).collect()
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
            .map(|(name, kind, mut times)| -> Result<StepStats> {
                let Some(kind) = StepKind::from_word(&kind) else {
                    fail!("{}: a step is of kind {kind:?}", path.display());
                };
                let ran = times.len() as u32;
                let slowest_ms = times.iter().copied().max().unwrap_or(0);
                let median_ms = median_of(&mut times);
                Ok(StepStats {
                    name: step_word(&name, kind),
                    kind,
                    ran,
                    median_ms,
                    slowest_ms,
                    total_ms: u64::from(ran) * median_ms,
                })
            })
            .collect::<Result<_>>()?;
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

#[cfg(test)]
#[path = "app_timing_tests.rs"]
mod tests;
