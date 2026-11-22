//! How often an app build's shots found what builds had saved: the run of
//! the merge base that a shots step compares with, and the screenshots the
//! step takes from the picture folder. Each shots step records both
//! (app_build_step_shots.rs). A key that changes on every run, or two hosts
//! whose environments differ, makes every lookup miss: each branch build then
//! builds the merge base again and shoots every screen, and nothing else says
//! so. `builds health` prints the rates over the last hour, day and week for
//! each repo, lane and environment, and lists a lane whose base runs were
//! saved for less than `LOW_SHARE` of the comparisons of the last day.

use std::collections::BTreeMap;

use crate::app_build_step_shots::BaseRun;
use crate::fail::Result;
use crate::health::Advice;
use crate::places::Places;
use crate::sccache_history::{DAY, WEEK, WINDOWS};
use crate::sccache_queue::Rate;

/// A lane's comparisons over the last day need at least this many saved
/// base runs, as a share of the saved and built ones, or health lists it.
const LOW_SHARE: f64 = 0.2;
/// Fewer comparisons than this in a day say nothing about the key.
const MIN_COMPARISONS: u64 = 5;

/// One shots step: when it started and what it used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub repo: String,
    pub lane: String,
    pub environment: String,
    /// Seconds since 1970.
    pub at: i64,
    pub base_run: BaseRun,
    /// The screenshots taken from the picture folder and the ones shot;
    /// None for a step that ended before its run finished.
    pub pictures: Option<(u32, u32)>,
}

/// The shots steps that started in the last week, in every project's store.
pub fn read(places: &Places, now: i64) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for path in crate::lists::store::database_paths(places)? {
        let Some(conn) = crate::db::open_reader(&path)? else {
            continue;
        };
        rows.extend(read_from(&conn, now, &path)?);
    }
    Ok(rows)
}

/// A stored row: repo, lane, environment, start, base run, reused and
/// captured.
type Stored = (
    String,
    String,
    String,
    i64,
    String,
    Option<u32>,
    Option<u32>,
);

fn read_from(conn: &rusqlite::Connection, now: i64, path: &std::path::Path) -> Result<Vec<Row>> {
    use crate::fail::Context;
    let what = || format!("reading {}", path.display());
    let raw: Vec<Stored> = conn
        .prepare(
            "SELECT r.repo, r.lane, s.environment, st.started_at, s.base_run, s.reused, s.captured
             FROM app_build_step_shots s
             JOIN app_build_steps st ON st.run_id = s.run_id AND st.position = s.position
             JOIN app_build_runs r ON r.id = s.run_id
             WHERE st.started_at > ?1",
        )
        .context(what)?
        .query_map([now - WEEK], |row| {
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
    raw.into_iter()
        .map(
            |(repo, lane, environment, at, base_run, reused, captured)| {
                Ok(Row {
                    repo,
                    lane,
                    environment,
                    at,
                    base_run: BaseRun::from_word(&base_run)?,
                    pictures: reused.zip(captured),
                })
            },
        )
        .collect()
}

/// The rows of one repo, lane and environment.
type Group = (String, String, String);

fn groups(rows: &[Row]) -> BTreeMap<Group, Vec<&Row>> {
    let mut found: BTreeMap<Group, Vec<&Row>> = BTreeMap::new();
    for row in rows {
        found
            .entry((row.repo.clone(), row.lane.clone(), row.environment.clone()))
            .or_default()
            .push(row);
    }
    found
}

/// The saved and built base runs of <rows> that started in the <window>
/// seconds before <now>: hits and misses.
fn base_rate(rows: &[&Row], now: i64, window: i64) -> Rate {
    let mut rate = Rate {
        since: None,
        hits: 0,
        misses: 0,
    };
    for row in rows.iter().filter(|row| row.at > now - window) {
        match row.base_run {
            BaseRun::Saved => rate.hits += 1,
            BaseRun::Built => rate.misses += 1,
            BaseRun::NotNeeded => {}
        }
    }
    rate
}

/// The screenshots of <rows> that started in the window: those taken from
/// the picture folder and those shot.
fn shots_rate(rows: &[&Row], now: i64, window: i64) -> Rate {
    let mut rate = Rate {
        since: None,
        hits: 0,
        misses: 0,
    };
    for (reused, captured) in rows
        .iter()
        .filter(|row| row.at > now - window)
        .filter_map(|row| row.pictures)
    {
        rate.hits += u64::from(reused);
        rate.misses += u64::from(captured);
    }
    rate
}

fn windows_text(rate: impl Fn(i64) -> Rate) -> String {
    WINDOWS
        .iter()
        .map(|(name, window)| {
            let share = rate(*window).share().unwrap_or_else(|| "none".to_string());
            format!("{name} {share}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The line `builds health` prints for each repo, lane and environment that
/// ran a shots step in the last week.
pub fn lines(rows: &[Row], now: i64) -> Vec<String> {
    groups(rows)
        .into_iter()
        .map(|((repo, lane, environment), rows)| {
            format!(
                "shots cache of {repo}'s {lane} lane in environment {environment}: base runs saved when a build looked for them: {}; screenshots taken from the picture folder: {}",
                windows_text(|window| base_rate(&rows, now, window)),
                windows_text(|window| shots_rate(&rows, now, window)),
            )
        })
        .collect()
}

/// The lines under "needs action": each lane whose builds of the last day
/// found the saved run of the merge base in less than `LOW_SHARE` of their
/// comparisons.
pub fn findings(rows: &[Row], now: i64) -> Vec<Advice> {
    groups(rows)
        .into_iter()
        .filter_map(|((repo, lane, environment), rows)| {
            let day = base_rate(&rows, now, DAY);
            let all = day.hits + day.misses;
            if all < MIN_COMPARISONS || day.hits as f64 >= all as f64 * LOW_SHARE {
                return None;
            }
            Some(Advice::said(format!(
                "shots cache: in the last day, builds found the saved run of the merge base for {} of the {all} comparisons of {repo}'s {lane} lane in environment {environment}, so they built the merge base again for the rest; check that the environment stays the same from one build to the next",
                day.hits
            )))
        })
        .collect()
}

#[cfg(test)]
#[path = "shots_hits_tests.rs"]
mod tests;
