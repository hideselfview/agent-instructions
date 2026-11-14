//! What a change did to the pre-commit hook's times: the recorded runs
//! (hook_stats.rs) split into those whose tree had the change and those
//! whose tree did not, with each check's figures on both sides.
//!
//! A run has a change when the commit HEAD pointed at when it started (the
//! run's `base`) contains it: `git merge-base --is-ancestor`. That is exact
//! where a split by time is not, because a worktree on an old base keeps
//! running the old code after the change landed. A run recorded before the
//! base was kept has no commit; it counts as "without" when it started before
//! the change was committed (its committer date, which for a change landed
//! fast-forward is when it reached origin/main) and is left out of both sides
//! when it started after, since it may or may not have had it. A run whose
//! base git no longer knows is left out too. Both are counted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::{Context, Result};
use crate::hook_stats::{self, median_of, open_for_reading, seconds};

/// One check's figures over one side's runs that ran it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckFigures {
    pub name: String,
    pub kind: String,
    pub ran: u32,
    pub median_ms: u64,
    pub slowest_ms: u64,
    /// The median of the workspace crates it was handed.
    pub median_crates: u64,
}

/// The runs on one side of a change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Side {
    pub runs: u32,
    pub median_ms: u64,
    pub slowest_ms: u64,
    /// By name, then kind.
    pub checks: Vec<CheckFigures>,
}

/// The split of the recorded runs by one change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Impact {
    /// The change, as the full commit id.
    pub change: String,
    /// When it was committed, seconds since 1970.
    pub committed_at: i64,
    /// Runs whose tree contains it.
    pub with: Side,
    /// Runs whose tree does not.
    pub without: Side,
    /// Of `without`, the runs with no recorded base, counted there for having
    /// started before the change was committed.
    pub without_by_time: u32,
    /// Runs with no recorded base that started after the change was
    /// committed.
    pub unplaced_by_time: u32,
    /// Runs whose base git does not know.
    pub unplaced_by_base: u32,
}

/// One run's facts for the split.
struct Facts {
    started_at: i64,
    base: Option<String>,
    total_ms: u64,
    /// Each check that ran: name, kind, time, crates handed.
    checks: Vec<(String, String, u64, u64)>,
}

/// Does <base> contain the change? None when git does not know <base>.
pub type Contains<'a> = &'a dyn Fn(&str) -> Option<bool>;

/// The split of the runs in the database at <path>. <contains> answers for a
/// run's base; <committed_at> is when the change was committed.
pub fn split(
    path: &Path,
    change: &str,
    committed_at: i64,
    contains: Contains,
) -> Result<Option<Impact>> {
    let Some(conn) = open_for_reading(path)? else {
        return Ok(None);
    };
    let facts = facts(&conn, path)?;
    let mut impact = Impact {
        change: change.to_string(),
        committed_at,
        with: Side::default(),
        without: Side::default(),
        without_by_time: 0,
        unplaced_by_time: 0,
        unplaced_by_base: 0,
    };
    let (mut with, mut without): (Vec<&Facts>, Vec<&Facts>) = (Vec::new(), Vec::new());
    let mut answers: HashMap<&str, Option<bool>> = HashMap::new();
    for run in &facts {
        match run.base.as_deref() {
            Some(base) => match *answers.entry(base).or_insert_with(|| contains(base)) {
                Some(true) => with.push(run),
                Some(false) => without.push(run),
                None => impact.unplaced_by_base += 1,
            },
            None if run.started_at < committed_at => {
                impact.without_by_time += 1;
                without.push(run);
            }
            None => impact.unplaced_by_time += 1,
        }
    }
    impact.with = side(&with);
    impact.without = side(&without);
    Ok(Some(impact))
}

fn facts(conn: &Connection, path: &Path) -> Result<Vec<Facts>> {
    let what = || format!("reading {}", path.display());
    let mut runs: Vec<(i64, Facts)> = Vec::new();
    let mut statement = conn
        .prepare(&"SELECT id, started_at, base, total_ms FROM hook_runs ORDER BY id")
        .context(what)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .context(what)?;
    for row in rows {
        let (id, started_at, base, total_ms) = row.context(what)?;
        runs.push((
            id,
            Facts {
                started_at,
                base,
                total_ms: total_ms as u64,
                checks: Vec::new(),
            },
        ));
    }
    let mut statement = conn
        .prepare("SELECT run_id, name, kind, ms, crates FROM hook_run_checks WHERE outcome <> 'skipped' ORDER BY run_id, position")
        .context(what)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .context(what)?;
    let index: HashMap<i64, usize> = runs
        .iter()
        .enumerate()
        .map(|(at, (id, _))| (*id, at))
        .collect();
    for row in rows {
        let (id, name, kind, ms, crates) = row.context(what)?;
        if let Some(&at) = index.get(&id) {
            runs[at]
                .1
                .checks
                .push((name, kind, ms as u64, crates as u64));
        }
    }
    Ok(runs.into_iter().map(|(_, facts)| facts).collect())
}

fn side(runs: &[&Facts]) -> Side {
    let mut totals: Vec<u64> = runs.iter().map(|r| r.total_ms).collect();
    let mut by_check: std::collections::BTreeMap<(&str, &str), (Vec<u64>, Vec<u64>)> =
        Default::default();
    for run in runs {
        for (name, kind, ms, crates) in &run.checks {
            let entry = by_check.entry((name, kind)).or_default();
            entry.0.push(*ms);
            entry.1.push(*crates);
        }
    }
    Side {
        runs: runs.len() as u32,
        median_ms: median_of(&mut totals),
        slowest_ms: totals.last().copied().unwrap_or(0),
        checks: by_check
            .into_iter()
            .map(|((name, kind), (mut times, mut crates))| CheckFigures {
                name: name.to_string(),
                kind: kind.to_string(),
                ran: times.len() as u32,
                median_ms: median_of(&mut times),
                slowest_ms: times.last().copied().unwrap_or(0),
                median_crates: median_of(&mut crates),
            })
            .collect(),
    }
}

/// A recorded worktree of the database at <path> that still exists, newest
/// run first: where git can be asked about the runs' commits, since every
/// worktree of a repo shares them.
pub fn repo_dir(path: &Path) -> Result<PathBuf> {
    let Some(conn) = open_for_reading(path)? else {
        fail!("{} holds no runs", path.display())
    };
    let what = || format!("reading {}", path.display());
    let mut statement = conn
        .prepare("SELECT worktree FROM hook_runs GROUP BY worktree ORDER BY MAX(started_at) DESC")
        .context(what)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .context(what)?;
    for row in rows {
        let dir = PathBuf::from(row.context(what)?);
        if dir.is_dir() {
            return Ok(dir);
        }
    }
    fail!(
        "none of the worktrees the runs in {} were made in exists any more",
        path.display()
    )
}

/// What a revision names, as a commit id and the time it was committed.
fn commit(repo: &Path, rev: &str) -> Result<(String, i64)> {
    if rev.is_empty() || rev.starts_with('-') {
        fail!("{rev:?} is not a revision");
    }
    let id = git_text(
        git(repo).args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ]),
        &format!("find {rev}"),
    )?;
    let id = id.trim().to_string();
    let at = git_text(
        git(repo).args(["show", "-s", "--format=%ct", &id]),
        &format!("read when {rev} was committed"),
    )?;
    let at = at
        .trim()
        .parse::<i64>()
        .context(|| format!("reading the commit time of {rev}"))?;
    Ok((id, at))
}

/// Does <base> contain <change>? None when git does not know <base>.
fn contains(repo: &Path, change: &str, base: &str) -> Option<bool> {
    let status = git(repo)
        .args(["merge-base", "--is-ancestor", change, base])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// The split of the runs in the database at <path> by <rev>, asking git in
/// the worktree of a recorded run.
pub fn impact(path: &Path, rev: &str) -> Result<Option<Impact>> {
    if open_for_reading(path)?.is_none() {
        return Ok(None);
    }
    let repo = repo_dir(path)?;
    let (change, committed_at) = commit(&repo, rev)?;
    split(path, &change, committed_at, &|base| {
        contains(&repo, &change, base)
    })
}

fn side_line(side: &Side) -> String {
    format!(
        "{} median {}, slowest {}",
        crate::text::counted(side.runs as usize, "run", "runs"),
        seconds(side.median_ms),
        seconds(side.slowest_ms)
    )
}

/// The text `builds checks stats --change` prints.
pub fn report(project: &str, impact: Option<&Impact>) -> String {
    use crate::text::local_minute;
    let Some(impact) = impact else {
        return format!("project {project}: no runs recorded\n");
    };
    let mut out = format!(
        "project {project}, runs split by {} (committed {})\n",
        &impact.change[..impact.change.len().min(12)],
        local_minute(impact.committed_at)
    );
    out.push_str(&format!(
        "  with the change:    {}\n",
        side_line(&impact.with)
    ));
    out.push_str(&format!(
        "  without the change: {}\n",
        side_line(&impact.without)
    ));
    if impact.without_by_time > 0 {
        out.push_str(&format!(
            "  {} of the runs without it were recorded before the hook kept its base commit; they count as without because they started before the change was committed\n",
            impact.without_by_time
        ));
    }
    if impact.unplaced_by_time > 0 {
        out.push_str(&format!(
            "  {} runs left out: recorded before the hook kept its base commit and started after the change was committed, so whether they had it is not known\n",
            impact.unplaced_by_time
        ));
    }
    if impact.unplaced_by_base > 0 {
        out.push_str(&format!(
            "  {} runs left out: their base commit is not in the repo any more\n",
            impact.unplaced_by_base
        ));
    }
    let names: std::collections::BTreeSet<(&str, &str)> = impact
        .with
        .checks
        .iter()
        .chain(&impact.without.checks)
        .map(|c| (c.name.as_str(), c.kind.as_str()))
        .collect();
    let width = names
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0)
        .max(5);
    out.push_str(&format!(
        "\n{:<width$}  {:<30}  {:<30}\n{:<width$}  {:<30}  {:<30}\n",
        "check",
        "with the change",
        "without the change",
        "",
        "ran  crates  median  slowest",
        "ran  crates  median  slowest"
    ));
    let cell = |c: Option<&CheckFigures>| match c {
        Some(c) => format!(
            "{:>3}  {:>6}  {:>6}  {:>7}",
            c.ran,
            c.median_crates,
            seconds(c.median_ms),
            seconds(c.slowest_ms)
        ),
        None => "-".to_string(),
    };
    for (name, kind) in names {
        let find = |side: &Side| {
            side.checks
                .iter()
                .find(|c| c.name == name && c.kind == kind)
                .cloned()
        };
        out.push_str(&format!(
            "{name:<width$}  {:<30}  {:<30}\n",
            cell(find(&impact.with).as_ref()),
            cell(find(&impact.without).as_ref())
        ));
    }
    out
}

fn side_json(side: &Side) -> serde_json::Value {
    serde_json::json!({
        "runs": side.runs, "median_ms": side.median_ms, "slowest_ms": side.slowest_ms,
        "checks": side.checks.iter().map(|c| serde_json::json!({
            "name": c.name, "kind": c.kind, "ran": c.ran, "median_ms": c.median_ms,
            "slowest_ms": c.slowest_ms, "median_crates": c.median_crates,
        })).collect::<Vec<_>>(),
    })
}

/// The dashboard's JSON for `/api/hooks/impact`: the split of <project>'s
/// runs by <rev>, or `{"error"}`.
pub fn impact_json(places: &crate::places::Places, project: &str, rev: &str) -> String {
    let value = (|| -> Result<serde_json::Value> {
        let stores = crate::lists::store::stores(places)?;
        let Some(store) = stores.iter().find(|store| store.project == project) else {
            fail!("no project {project:?}")
        };
        let Some(impact) = impact(&store.dir.join(hook_stats::DATABASE), rev)? else {
            return Ok(serde_json::json!({ "project": project, "runs": 0 }));
        };
        Ok(serde_json::json!({
            "project": project, "runs": impact.with.runs + impact.without.runs,
            "change": impact.change, "committed_at": impact.committed_at,
            "with": side_json(&impact.with), "without": side_json(&impact.without),
            "without_by_time": impact.without_by_time, "unplaced_by_time": impact.unplaced_by_time,
            "unplaced_by_base": impact.unplaced_by_base,
        }))
    })();
    value
        .unwrap_or_else(|error| serde_json::json!({ "error": error.text() }))
        .to_string()
}

#[cfg(test)]
#[path = "hook_impact_tests.rs"]
mod tests;
