//! The record of every pre-commit hook run: `builds checks run` writes one
//! run, with its staged files and every check, to `builds.sqlite` in the
//! project's store folder (the schema is hook_stats.sql). `builds checks
//! stats` and the dashboard read it back; the figures are SQL queries over the
//! rows, never stored.
//!
//! Writing is best effort for the commit and loud for the person: the caller
//! prints a warning when a record fails and the hook's result is the checks'
//! alone. A run is written in one transaction, whole or not at all. Reading
//! opens the file read-only and never makes it; a file with a schema this
//! code does not know is an error.
//!
//! Each run records the commit HEAD pointed at when it started (`base`, the
//! last column of `hook_runs`, in the layout an ALTER TABLE makes, closing `)` and all, so the move of an
//! older store in migrate.rs leaves a schema equal to a new one's; NULL for a
//! repo with no commit and for a run made before the column), so
//! what a change did to the hook's times is exact (hook_impact.rs): a run has
//! a change when its base contains it. The figures also carry, per check, the
//! trend of its latest runs against the ones before them, and the check sets
//! whose newest run went over the hook's time limit (`LIMIT_MS`), which the
//! dashboard shows among the things needing action.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::checks::Kind;
use crate::fail;
use crate::fail::{Context, Result};

pub use crate::db::DATABASE;

/// What a commit hook may take: the rule `a-commit-hook-formats-and-statically-
/// checks-what-changed` gives it 45 seconds.
pub const LIMIT_MS: u64 = 45_000;
/// How many of a check's latest runs the trend takes the median of, and how
/// many before them it is set against.
pub const TREND_RUNS: usize = 10;
/// The fewest earlier runs a trend is shown against.
const TREND_EARLIER_MIN: usize = 3;

/// How a staged file differs from HEAD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

impl Change {
    pub fn word(self) -> &'static str {
        match self {
            Change::Added => "added",
            Change::Modified => "modified",
            Change::Deleted => "deleted",
        }
    }

    fn from_word(word: &str) -> Option<Change> {
        [Change::Added, Change::Modified, Change::Deleted]
            .into_iter()
            .find(|c| c.word() == word)
    }
}

/// One file of the staged diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedFile {
    pub path: String,
    pub change: Change,
    /// Lines added and removed; None for a binary file.
    pub lines: Option<(u64, u64)>,
}

impl StagedFile {
    /// The part of the name after its last dot, or `(none)`.
    pub fn extension(&self) -> String {
        match Path::new(&self.path).extension() {
            Some(extension) => extension.to_string_lossy().into_owned(),
            None => "(none)".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
}

impl Outcome {
    fn word(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed => "failed",
            Outcome::Skipped => "skipped",
        }
    }

    fn from_word(word: &str) -> Option<Outcome> {
        [Outcome::Passed, Outcome::Failed, Outcome::Skipped]
            .into_iter()
            .find(|o| o.word() == word)
    }
}

/// What a check or fix of `.checks` did in one run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckRecord {
    pub name: String,
    /// `Kind::Check` or `Kind::Fix`.
    pub kind: Kind,
    pub outcome: Outcome,
    /// The staged files and the workspace crates it was handed, and its
    /// time; None for a skipped one.
    pub handed: Option<(u32, u32)>,
    pub ms: Option<u64>,
}

/// One hook run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Seconds since 1970 when it started.
    pub at: i64,
    pub repo: String,
    pub worktree: String,
    /// None for a detached HEAD.
    pub branch: Option<String>,
    /// The commit HEAD pointed at when it started; None for a repo with no
    /// commit and for a run recorded before the column existed.
    pub base: Option<String>,
    pub passed: bool,
    pub plan_ms: u64,
    pub total_ms: u64,
    pub files: Vec<StagedFile>,
    /// In the order of `.checks`.
    pub checks: Vec<CheckRecord>,
}

impl Run {
    pub fn added(&self) -> u64 {
        self.files
            .iter()
            .filter_map(|f| f.lines)
            .map(|(added, _)| added)
            .sum()
    }

    pub fn removed(&self) -> u64 {
        self.files
            .iter()
            .filter_map(|f| f.lines)
            .map(|(_, removed)| removed)
            .sum()
    }
}

fn kind_of(word: &str) -> Result<Kind> {
    match word {
        "check" => Ok(Kind::Check),
        "fix" => Ok(Kind::Fix),
        other => fail!("the database holds a check of kind {other:?}"),
    }
}

/// The database at <path> ready for writing: made with its schema when it is
/// new. The folder must exist; this never makes it.
fn open_for_writing(path: &Path) -> Result<Connection> {
    crate::db::open(path, crate::db::Make::Allowed)
}

/// The database at <path> for reading; None when there is no file or no
/// schema in it yet.
pub(crate) fn open_for_reading(path: &Path) -> Result<Option<Connection>> {
    crate::db::open_reader(path)
}

/// Writes <run>, its files and its checks to the database at <path>, in one
/// transaction.
pub fn record(path: &Path, run: &Run) -> Result<()> {
    let mut conn = open_for_writing(path)?;
    let what = || format!("writing {}", path.display());
    let tx = conn.transaction().context(what)?;
    tx.execute(
        "INSERT INTO hook_runs (started_at, repo, worktree, branch, base, passed, plan_ms, total_ms) VALUES (?1, ?2, ?3, ?4, ?8, ?5, ?6, ?7)",
        params![run.at, run.repo, run.worktree, run.branch, run.passed, run.plan_ms as i64, run.total_ms as i64, run.base],
    )
    .context(what)?;
    let id = tx.last_insert_rowid();
    {
        let mut file = tx
            .prepare("INSERT INTO hook_run_files (run_id, path, change, added, removed) VALUES (?1, ?2, ?3, ?4, ?5)")
            .context(what)?;
        for f in &run.files {
            let (added, removed) = f.lines.map(|(a, r)| (a as i64, r as i64)).unzip();
            file.execute(params![id, f.path, f.change.word(), added, removed])
                .context(what)?;
        }
        let mut check = tx
            .prepare("INSERT INTO hook_run_checks (run_id, position, name, kind, outcome, files, crates, ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
            .context(what)?;
        for (position, c) in run.checks.iter().enumerate() {
            let (files, crates) = c.handed.map(|(f, k)| (f as i64, k as i64)).unzip();
            check
                .execute(params![
                    id,
                    position as i64,
                    c.name,
                    c.kind.word(),
                    c.outcome.word(),
                    files,
                    crates,
                    c.ms.map(|ms| ms as i64)
                ])
                .context(what)?;
        }
    }
    tx.commit().context(what)
}

/// One check's figures over a set of runs.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckStats {
    pub name: String,
    pub kind: Kind,
    pub ran: u32,
    pub failed: u32,
    /// Runs in which the staged files did not pick it.
    pub skipped: u32,
    pub median_ms: u64,
    pub slowest_ms: u64,
    /// Its latest runs against the ones before; None until enough ran.
    pub trend: Option<Trend>,
}

/// The median time of a check's latest `TREND_RUNS` runs and of the
/// `TREND_RUNS` before them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trend {
    pub recent_ms: u64,
    pub earlier_ms: u64,
}

/// The median of <times>, which it sorts.
pub(crate) fn median_of(times: &mut [u64]) -> u64 {
    times.sort_unstable();
    match times.len() {
        0 => 0,
        n if n % 2 == 1 => times[n / 2],
        n => ((times[n / 2 - 1] + times[n / 2]) as f64 / 2.0).round() as u64,
    }
}

/// The figures over the runs since some moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub runs: u32,
    pub failed: u32,
    pub median_ms: u64,
    pub slowest_ms: u64,
    /// By median time, slowest first.
    pub checks: Vec<CheckStats>,
    /// The longest runs, longest first.
    pub slowest: Vec<Run>,
    /// The latest runs, newest first.
    pub recent: Vec<Run>,
    /// Of every set of checks that ran, the newest run when it took over
    /// `LIMIT_MS`, longest first: what needs acting on. A newer run of the
    /// same set under the limit ends it (`over_limit`).
    pub over_limit: Vec<Run>,
}

/// How many of the longest runs `Stats` holds.
pub const SLOWEST_RUNS: usize = 5;
/// How many of the latest runs `Stats` holds.
pub const RECENT_RUNS: usize = 20;

/// The condition that keeps the middle row of a group, or the two middle
/// ones whose mean is the median: `n` is a row's place in the group's order and
/// `c` the group's size.
const MIDDLE: &str = "n IN ((c + 1) / 2, (c + 2) / 2)";

fn rounded(mean: Option<f64>) -> u64 {
    mean.map_or(0, |mean| mean.round() as u64)
}

/// The figures over the runs of the database at <path> that started at or
/// after <since> (seconds since 1970); None when nothing was ever recorded
/// there.
pub fn stats(path: &Path, since: i64) -> Result<Option<Stats>> {
    let Some(conn) = open_for_reading(path)? else {
        return Ok(None);
    };
    let what = || format!("reading {}", path.display());
    let (runs, failed, slowest_ms): (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(passed = 0), 0), COALESCE(MAX(total_ms), 0) FROM hook_runs WHERE started_at >= ?1",
            [since],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .context(what)?;
    let median: Option<f64> = conn
        .query_row(
            &format!(
                "SELECT AVG(total_ms) FROM (
                    SELECT total_ms, ROW_NUMBER() OVER (ORDER BY total_ms) AS n, COUNT(*) OVER () AS c
                    FROM hook_runs WHERE started_at >= ?1)
                 WHERE {MIDDLE}"
            ),
            [since],
            |row| row.get(0),
        )
        .context(what)?;
    let mut statement = conn
        .prepare(&format!(
            "WITH timed AS (
                SELECT c.name, c.kind, c.ms,
                       ROW_NUMBER() OVER (PARTITION BY c.name, c.kind ORDER BY c.ms) AS n,
                       COUNT(*) OVER (PARTITION BY c.name, c.kind) AS c
                FROM hook_run_checks c JOIN hook_runs r ON r.id = c.run_id
                WHERE r.started_at >= ?1 AND c.outcome <> 'skipped'),
             medians AS (SELECT name, kind, AVG(ms) AS median FROM timed WHERE {MIDDLE} GROUP BY name, kind)
             SELECT c.name, c.kind,
                    COALESCE(SUM(c.outcome <> 'skipped'), 0), COALESCE(SUM(c.outcome = 'failed'), 0),
                    COALESCE(SUM(c.outcome = 'skipped'), 0), COALESCE(MAX(c.ms), 0), m.median
             FROM hook_run_checks c JOIN hook_runs r ON r.id = c.run_id
             LEFT JOIN medians m ON m.name = c.name AND m.kind = c.kind
             WHERE r.started_at >= ?1
             GROUP BY c.name, c.kind
             ORDER BY COALESCE(m.median, 0) DESC, c.name"
        ))
        .context(what)?;
    let rows = statement
        .query_map([since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<f64>>(6)?,
            ))
        })
        .context(what)?;
    let mut checks = Vec::new();
    for row in rows {
        let (name, kind, ran, failed, skipped, slowest_ms, median) = row.context(what)?;
        checks.push(CheckStats {
            name,
            kind: kind_of(&kind)?,
            ran: ran as u32,
            failed: failed as u32,
            skipped: skipped as u32,
            median_ms: rounded(median),
            slowest_ms: slowest_ms as u64,
            trend: None,
        });
    }
    let trends = trends(&conn, since, path)?;
    for c in &mut checks {
        c.trend = trends
            .get(&(c.name.clone(), c.kind.word().to_string()))
            .copied();
    }
    let slowest = runs_where(
        &conn,
        "started_at >= ?1 ORDER BY total_ms DESC, started_at DESC",
        since,
        SLOWEST_RUNS,
        path,
    )?;
    let recent = runs_where(
        &conn,
        "started_at >= ?1 ORDER BY started_at DESC, id DESC",
        since,
        RECENT_RUNS,
        path,
    )?;
    let over_limit = over_limit(&conn, since, path)?;
    Ok(Some(Stats {
        runs: runs as u32,
        failed: failed as u32,
        median_ms: rounded(median),
        slowest_ms: slowest_ms as u64,
        checks,
        slowest,
        recent,
        over_limit,
    }))
}

/// The trend of each check (by name and kind) that ran enough times since
/// <since>: its latest runs against the ones before them.
fn trends(
    conn: &Connection,
    since: i64,
    path: &Path,
) -> Result<std::collections::HashMap<(String, String), Trend>> {
    let what = || format!("reading {}", path.display());
    let mut statement = conn
        .prepare(
            "SELECT c.name, c.kind, c.ms FROM hook_run_checks c JOIN hook_runs r ON r.id = c.run_id
             WHERE r.started_at >= ?1 AND c.outcome <> 'skipped' ORDER BY r.started_at DESC, r.id DESC",
        )
        .context(what)?;
    let rows = statement
        .query_map([since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .context(what)?;
    let mut times: std::collections::HashMap<(String, String), Vec<u64>> =
        std::collections::HashMap::new();
    for row in rows {
        let (name, kind, ms) = row.context(what)?;
        times.entry((name, kind)).or_default().push(ms as u64);
    }
    let mut out = std::collections::HashMap::new();
    for (key, all) in times {
        let (recent, earlier) = all.split_at(all.len().min(TREND_RUNS));
        let earlier = &earlier[..earlier.len().min(TREND_RUNS)];
        if earlier.len() >= TREND_EARLIER_MIN {
            out.insert(
                key,
                Trend {
                    recent_ms: median_of(&mut recent.to_vec()),
                    earlier_ms: median_of(&mut earlier.to_vec()),
                },
            );
        }
    }
    Ok(out)
}

/// Groups the runs since <since> by the set of checks that ran in them, and
/// returns each group's newest run when it took over `LIMIT_MS`, longest
/// first. A set stays flagged until a newer run of it is under the limit,
/// which is when its newest run is no longer over.
fn over_limit(conn: &Connection, since: i64, path: &Path) -> Result<Vec<Run>> {
    let what = || format!("reading {}", path.display());
    let mut statement = conn
        .prepare(
            "SELECT r.id, r.total_ms, c.name FROM hook_runs r LEFT JOIN hook_run_checks c ON c.run_id = r.id AND c.outcome <> 'skipped'
             WHERE r.started_at >= ?1 ORDER BY r.started_at DESC, r.id DESC",
        )
        .context(what)?;
    let rows = statement
        .query_map([since], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .context(what)?;
    // Runs newest first, each with the names of the checks that ran.
    let mut runs: Vec<(i64, u64, std::collections::BTreeSet<String>)> = Vec::new();
    for row in rows {
        let (id, total, name) = row.context(what)?;
        if runs.last().map(|r| r.0) != Some(id) {
            runs.push((id, total as u64, Default::default()));
        }
        if let Some(name) = name {
            runs.last_mut().expect("just pushed").2.insert(name);
        }
    }
    let mut seen: std::collections::HashSet<std::collections::BTreeSet<String>> =
        Default::default();
    let mut flagged: Vec<(i64, u64)> = Vec::new();
    for (id, total, set) in runs {
        if set.is_empty() || !seen.insert(set) {
            continue;
        }
        if total > LIMIT_MS {
            flagged.push((id, total));
        }
    }
    if flagged.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = flagged.iter().map(|(id, _)| id.to_string()).collect();
    runs_where(
        conn,
        &format!(
            "id IN ({}) AND started_at >= ?1 ORDER BY total_ms DESC, started_at DESC",
            ids.join(",")
        ),
        since,
        flagged.len(),
        path,
    )
}

/// The first <limit> runs of `hook_runs WHERE <clause>` (which takes
/// `?1` as the earliest start), each with its files and checks.
fn runs_where(
    conn: &Connection,
    clause: &str,
    since: i64,
    limit: usize,
    path: &Path,
) -> Result<Vec<Run>> {
    let what = || format!("reading {}", path.display());
    let mut statement = conn
        .prepare(&format!(
            "SELECT id, started_at, repo, worktree, branch, passed, plan_ms, total_ms, base FROM hook_runs WHERE {clause} LIMIT ?2"
        ))
        .context(what)?;
    let heads = statement
        .query_map(params![since, limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                Run {
                    at: row.get(1)?,
                    repo: row.get(2)?,
                    worktree: row.get(3)?,
                    branch: row.get(4)?,
                    passed: row.get(5)?,
                    plan_ms: row.get::<_, i64>(6)? as u64,
                    total_ms: row.get::<_, i64>(7)? as u64,
                    base: row.get(8)?,
                    files: Vec::new(),
                    checks: Vec::new(),
                },
            ))
        })
        .context(what)?;
    let mut runs = Vec::new();
    for head in heads {
        let (id, mut run) = head.context(what)?;
        let mut files = conn
            .prepare("SELECT path, change, added, removed FROM hook_run_files WHERE run_id = ?1 ORDER BY path")
            .context(what)?;
        let rows = files
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })
            .context(what)?;
        for row in rows {
            let (path, change, added, removed) = row.context(what)?;
            let Some(change) = Change::from_word(&change) else {
                fail!("the database holds a file change {change:?}")
            };
            run.files.push(StagedFile {
                path,
                change,
                lines: added.zip(removed).map(|(a, r)| (a as u64, r as u64)),
            });
        }
        let mut checks = conn
            .prepare("SELECT name, kind, outcome, files, crates, ms FROM hook_run_checks WHERE run_id = ?1 ORDER BY position")
            .context(what)?;
        let rows = checks
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })
            .context(what)?;
        for row in rows {
            let (name, kind, outcome, files, crates, ms) = row.context(what)?;
            let Some(outcome) = Outcome::from_word(&outcome) else {
                fail!("the database holds a check outcome {outcome:?}")
            };
            run.checks.push(CheckRecord {
                name,
                kind: kind_of(&kind)?,
                outcome,
                handed: files.zip(crates).map(|(f, k)| (f as u32, k as u32)),
                ms: ms.map(|ms| ms as u64),
            });
        }
        runs.push(run);
    }
    Ok(runs)
}

/// <ms> as `12.3s`.
pub fn seconds(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

fn kinds_of(run: &Run) -> std::collections::BTreeMap<String, u32> {
    let mut kinds = std::collections::BTreeMap::new();
    for file in &run.files {
        *kinds.entry(file.extension()).or_default() += 1;
    }
    kinds
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The text `builds checks stats` prints: <stats> of <project>, read since
/// <since> (a day, for the heading).
pub fn report(project: &str, since: Option<&str>, stats: Option<&Stats>) -> String {
    use crate::text::local_minute;
    let heading = match since {
        Some(day) => format!("project {project}, runs since {day}"),
        None => format!("project {project}, all runs"),
    };
    let Some(stats) = stats.filter(|stats| stats.runs > 0) else {
        return format!("{heading}: none recorded\n");
    };
    let mut out = format!(
        "{heading}: {}, {} failed, median {}, slowest {}\n\n",
        plural(stats.runs, "run", "runs"),
        stats.failed,
        seconds(stats.median_ms),
        seconds(stats.slowest_ms)
    );
    let width = stats
        .checks
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(0)
        .max(5);
    out.push_str(&format!("{:<width$}  kind   ran  failed  skipped  median  slowest  latest {TREND_RUNS} vs the {TREND_RUNS} before\n", "check"));
    for c in &stats.checks {
        out.push_str(&format!(
            "{:<width$}  {:<5} {:>4}  {:>6}  {:>7}  {:>6}  {:>7}  {}\n",
            c.name,
            c.kind.word(),
            c.ran,
            c.failed,
            c.skipped,
            seconds(c.median_ms),
            seconds(c.slowest_ms),
            c.trend.map_or(String::new(), |t| format!(
                "{} vs {}",
                seconds(t.recent_ms),
                seconds(t.earlier_ms)
            ))
        ));
    }
    if !stats.over_limit.is_empty() {
        out.push_str(&format!("\nover the {} limit (the newest run of each set of checks; a newer run of the set under the limit ends it):\n", seconds(LIMIT_MS)));
        for run in &stats.over_limit {
            out.push_str(&format!(
                "{}  {:>7}  {}  {}\n",
                local_minute(run.at),
                seconds(run.total_ms),
                run.worktree,
                handed_text(run)
            ));
        }
    }
    out.push_str("\nslowest runs:\n");
    for run in &stats.slowest {
        let kinds: Vec<String> = kinds_of(run)
            .iter()
            .map(|(kind, n)| format!("{kind} {n}"))
            .collect();
        out.push_str(&format!(
            "{}  {:>7}  {}  {} +{} -{} ({})  {}\n",
            local_minute(run.at),
            seconds(run.total_ms),
            run.branch.as_deref().unwrap_or("(detached)"),
            plural(run.files.len() as u32, "file", "files"),
            run.added(),
            run.removed(),
            kinds.join(", "),
            if run.passed { "passed" } else { "failed" }
        ));
    }
    out
}

/// What a run was handed and where its time went: `12 files, 3 crates;
/// slowest clippy 38.1s, ownership 10.4s`.
pub fn handed_text(run: &Run) -> String {
    let (files, crates) = handed_of(run);
    let mut ran: Vec<&CheckRecord> = run.checks.iter().filter(|c| c.ms.is_some()).collect();
    ran.sort_by_key(|c| std::cmp::Reverse(c.ms));
    let slowest: Vec<String> = ran
        .iter()
        .take(3)
        .map(|c| format!("{} {}", c.name, seconds(c.ms.unwrap_or(0))))
        .collect();
    format!(
        "{}, {}; slowest {}",
        plural(files, "file", "files"),
        plural(crates, "crate", "crates"),
        slowest.join(", ")
    )
}

/// The files the run's checks were handed, and the most crates any was.
fn handed_of(run: &Run) -> (u32, u32) {
    let files = run.files.len() as u32;
    (
        files,
        run.checks
            .iter()
            .filter_map(|c| c.handed)
            .map(|(_, crates)| crates)
            .max()
            .unwrap_or(0),
    )
}

pub(crate) fn run_json(run: &Run) -> serde_json::Value {
    serde_json::json!({
        "at": run.at,
        "repo": run.repo,
        "worktree": run.worktree,
        "branch": run.branch,
        "base": run.base,
        "passed": run.passed,
        "plan_ms": run.plan_ms,
        "total_ms": run.total_ms,
        "files": run.files.len(),
        "added": run.added(),
        "removed": run.removed(),
        "kinds": kinds_of(run),
        "crates": handed_of(run).1,
        "checks": run.checks.iter().map(|c| serde_json::json!({
            "name": c.name, "kind": c.kind.word(), "outcome": c.outcome.word(), "ms": c.ms, "crates": c.handed.map(|(_, k)| k),
        })).collect::<Vec<_>>(),
    })
}

/// The dashboard's JSON for one project: its figures and latest runs, or
/// `error` when the database can't be read.
fn project_json(project: &str, path: &Path) -> serde_json::Value {
    match stats(path, 0) {
        Ok(None) => serde_json::json!({ "project": project, "runs": 0 }),
        Ok(Some(stats)) => serde_json::json!({
            "project": project,
            "runs": stats.runs,
            "failed": stats.failed,
            "median_ms": stats.median_ms,
            "slowest_ms": stats.slowest_ms,
            "checks": stats.checks.iter().map(|c| serde_json::json!({
                "name": c.name, "kind": c.kind.word(), "ran": c.ran, "failed": c.failed,
                "skipped": c.skipped, "median_ms": c.median_ms, "slowest_ms": c.slowest_ms,
                "recent_ms": c.trend.map(|t| t.recent_ms), "earlier_ms": c.trend.map(|t| t.earlier_ms),
            })).collect::<Vec<_>>(),
            "trend_runs": TREND_RUNS,
            "limit_ms": LIMIT_MS,
            "over_limit": stats.over_limit.iter().map(run_json).collect::<Vec<_>>(),
            "slowest": stats.slowest.iter().map(run_json).collect::<Vec<_>>(),
            "recent": stats.recent.iter().map(run_json).collect::<Vec<_>>(),
        }),
        Err(error) => serde_json::json!({ "project": project, "error": error.text() }),
    }
}

/// The dashboard's JSON for every project with a store: `{"projects": [...]}`,
/// or `{"error"}` when the stores' folder can't be listed.
pub fn dashboard_json(places: &crate::places::Places) -> String {
    let value = match crate::lists::store::stores(places) {
        Ok(stores) => {
            let projects: Vec<serde_json::Value> = stores
                .iter()
                .map(|store| project_json(&store.project, &store.dir.join(DATABASE)))
                .collect();
            serde_json::json!({ "projects": projects })
        }
        Err(error) => serde_json::json!({ "error": error.text() }),
    };
    value.to_string()
}

#[cfg(test)]
#[path = "hook_stats_tests.rs"]
mod tests;
