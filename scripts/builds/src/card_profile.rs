//! Where a card's time went, from ready to landed. `builds item times N`
//! prints a card's timeline, and `builds item times` with no id prints the
//! time by kind and the slowest steps over the cards that landed lately.
//!
//! Nothing here is stored. A card's moments come from its item events, runs
//! and spans (lists/), and its steps from the rows the other recorders keep
//! for the worktrees and branch its runs used: the cargo runs, hook runs, app
//! builds, review runs and landings, each with the host's samples over its
//! span (host_sample.rs). `attribute` gives every second of the card's time
//! to one kind, the highest of the kinds that cover it, so the kinds add up
//! to the card's time.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::hook_stats::seconds;
use crate::host::LOCAL;
use crate::lists::item_event::{self, Kind as EventKind};
use crate::lists::{Closed, Id, Item, Lists, Record, SpanKind};
use crate::review::short;
use crate::step_failure::{RunResult, StepFailure};
use crate::step_outcomes::{Attempt, Finding, StepKind};
use crate::text::{counted, duration, local_second};

/// How many days back the totals reach.
pub const RECENT_DAYS: i64 = 14;

/// What a stretch of a card's time was spent on, the highest priority first.
/// A moment that two stretches cover belongs to the one that comes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// The item waited on a question or a design change the person had not
    /// answered.
    WaitingOnYou,
    /// The worktree's claim note said held.
    Held,
    /// `builds land`.
    Landing,
    /// A review run's reviewers, or the dashboard's Reviewing.
    Review,
    /// The dashboard's Findings to settle: findings wait for the coordinator.
    WaitingForSettle,
    /// The dashboard's Open findings: confirmed findings wait for the fix
    /// and the review that closes them.
    Fixing,
    /// An app build's wait in its lane's queue.
    AppBuildQueue,
    /// A step of an app build.
    AppBuildStep,
    /// A commit hook run.
    Hook,
    /// A `builds cargo` run no hook check started.
    Cargo,
    /// The dashboard's Needs review and Unreviewed changes: commits wait for
    /// the coordinator to run the review.
    WaitingForReview,
    /// The dashboard's Ready to land: commits wait for `builds land`.
    WaitingToLand,
    /// An agent's run, in none of the kinds above.
    Working,
    /// The time between two runs of the item.
    BetweenRuns,
    /// The item was ready and no agent had taken it.
    Queued,
    /// The item waited on other work to start.
    Blocked,
    /// The store recorded no state for the time.
    NotRecorded,
}

impl Kind {
    pub fn words(self) -> &'static str {
        match self {
            Kind::WaitingOnYou => "waiting on the person",
            Kind::Held => "held by its agent",
            Kind::Landing => "landing",
            Kind::Review => "reviewers running",
            Kind::WaitingForSettle => "waiting for findings to be settled",
            Kind::Fixing => "fixing review findings",
            Kind::AppBuildQueue => "app build queue",
            Kind::AppBuildStep => "app build steps",
            Kind::Hook => "commit hooks",
            Kind::Cargo => "cargo runs",
            Kind::WaitingForReview => "waiting for a review",
            Kind::WaitingToLand => "ready, waiting for builds land",
            Kind::Working => "agent working",
            Kind::BetweenRuns => "between runs",
            Kind::Queued => "ready, not taken",
            Kind::Blocked => "blocked by other work",
            Kind::NotRecorded => "before builds recorded states",
        }
    }
}

/// A stretch of time in a card's life.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub kind: Kind,
    /// What it was, for example `cargo nextest run` or `review Errors`.
    pub name: String,
    /// Seconds since 1970.
    pub start: i64,
    pub end: i64,
    /// Where it ran, when it ran on a host.
    pub host: Option<String>,
    pub detail: String,
    /// Does the time by kind count it? False for a step inside another, such
    /// as the cargo run a hook check started.
    pub attributed: bool,
    /// Does the table of the slowest steps count it? False for a step that
    /// only holds others, such as a hook run, whose checks are counted.
    pub leaf: bool,
}

/// A moment of a card's life that the timeline lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moment {
    pub at: i64,
    pub words: String,
}

/// What the store recorded of a card.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: Id,
    pub title: String,
    /// From the card's opening to its landing, or to now while it is open.
    pub from: i64,
    pub to: i64,
    pub moments: Vec<Moment>,
    pub steps: Vec<Step>,
    /// Each run of an agent, from its start to its end or now.
    pub runs: Vec<(i64, i64)>,
    /// The item's state events that say whether it was ready or blocked.
    pub states: Vec<(i64, Kind)>,
    /// Each run of a hook check, a cargo run or an app build step that passed
    /// or failed, in time order.
    pub attempts: Vec<Attempt>,
    /// Each finding of the card's review runs.
    pub findings: Vec<Finding>,
    /// Each stretch of dashboard state of the cards that listed the item
    /// (card_state.rs), cut to the card's time.
    pub dashboard: Vec<crate::card_state::Stretch>,
    /// Each step that ran on a commit off its run's branch, with when it
    /// started: on neither that branch nor origin/main since the branch left
    /// it, as a bisect or an old checkout does.
    pub off_branch: Vec<(i64, String)>,
}

fn find(lists: &Lists, id: Id) -> Result<(&Item, Option<&Closed>)> {
    if let Some(item) = lists.item(id) {
        return Ok((item, None));
    }
    match lists.closed_record(id) {
        Some(closed) => match &closed.record {
            Record::Item(item) => Ok((item, Some(closed))),
            other => fail!("{id} is a {}, not an item", other.kind().name()),
        },
        None => fail!("{id} is not an item"),
    }
}

/// A run of an item: when it started and ended (None while it goes on), and
/// where it worked.
struct RunPlaces {
    since: i64,
    ended: Option<i64>,
    /// A repo, a worktree and a branch for each place.
    places: Vec<(String, String, String)>,
}

/// The runs of <item>, the ended ones first, then the one that goes on.
fn runs_of(item: &Item) -> Result<Vec<RunPlaces>> {
    let mut runs: Vec<RunPlaces> = item
        .runs
        .iter()
        .map(|run| RunPlaces {
            since: run.since.at,
            ended: Some(run.ended.at),
            places: run
                .places
                .iter()
                .map(|p| (p.repo.clone(), p.worktree.clone(), p.branch.clone()))
                .collect(),
        })
        .collect();
    if let Some(first) = item.running.first() {
        runs.push(RunPlaces {
            since: first.since.at,
            ended: None,
            places: item
                .running
                .iter()
                .map(|running| {
                    let Some((repo, _)) = crate::lists::side_worktree_parts(&running.worktree)
                    else {
                        fail!("{} is no side worktree", running.worktree);
                    };
                    Ok((repo, running.worktree.clone(), running.branch.clone()))
                })
                .collect::<Result<_>>()?,
        });
    }
    Ok(runs)
}

/// <host> for a checkout entry of an app build lane: the host an entry
/// `host:name` names, else this Mac.
fn host_of_checkout(entry: &str) -> String {
    crate::app_remote::split(entry).map_or_else(|| LOCAL.to_string(), |(host, _)| host.to_string())
}

/// Reads what the store recorded of item <id> at <now>.
pub fn gather(db: &Connection, lists: &Lists, id: Id, now: i64) -> Result<Card> {
    let (item, closed) = find(lists, id)?;
    let to = closed
        .map_or(now, |closed| closed.closed.at)
        .max(item.opened.at);
    let mut card = Card {
        id,
        title: item.title.clone(),
        from: item.opened.at,
        to,
        moments: Vec::new(),
        steps: Vec::new(),
        runs: Vec::new(),
        states: Vec::new(),
        attempts: Vec::new(),
        findings: Vec::new(),
        dashboard: Vec::new(),
        off_branch: Vec::new(),
    };
    card.moments.push(Moment {
        at: item.opened.at,
        words: format!("opened by {}", item.opened.by),
    });
    for event in item_event::read_item(db, id)? {
        let words = match (event.kind, &event.detail) {
            (EventKind::Ready, _) => "ready".to_string(),
            (EventKind::Blocked, Some(why)) => format!("blocked: {why}"),
            (EventKind::Blocked, None) => "blocked".to_string(),
            (EventKind::Running, _) => "taken".to_string(),
            (EventKind::WaitGained, Some(wait)) => format!("gained the wait {wait}"),
            (EventKind::WaitLost, Some(wait)) => format!("lost the wait {wait}"),
            (EventKind::HandedOff, Some(brief)) => format!("handed to the brief {brief}"),
            (kind, _) => kind.word().to_string(),
        };
        match event.kind {
            EventKind::Ready => card.states.push((event.at, Kind::Queued)),
            EventKind::Blocked => card.states.push((event.at, Kind::Blocked)),
            _ => {}
        }
        card.moments.push(Moment {
            at: event.at,
            words,
        });
    }
    for RunPlaces {
        since,
        ended,
        places,
    } in runs_of(item)?
    {
        let end = ended.unwrap_or(to.max(since));
        card.runs.push((since, end));
        card.moments.push(Moment {
            at: since,
            words: "an agent started a run".to_string(),
        });
        if let Some(ended) = ended {
            card.moments.push(Moment {
                at: ended,
                words: "the run ended".to_string(),
            });
        }
        for (repo, worktree, branch) in places {
            read_cargo(db, &worktree, since, end, &mut card)?;
            read_hooks(db, &worktree, &branch, since, end, &mut card)?;
            read_app_builds(db, &repo, &branch, since, end, &mut card)?;
            read_reviews(db, &repo, &branch, since, end, &mut card)?;
        }
    }
    read_landings(db, id, &mut card)?;
    read_dashboard(db, id, &mut card)?;
    for span in &item.spans {
        let (kind, words) = match span.kind {
            SpanKind::Held => (Kind::Held, "held by its agent"),
            SpanKind::Waiting => (Kind::WaitingOnYou, "waiting on the person"),
        };
        let end = span.ended.unwrap_or(to);
        card.moments.push(Moment {
            at: span.since,
            words: format!("began {words}"),
        });
        if span.ended.is_some() {
            card.moments.push(Moment {
                at: end,
                words: format!("ended {words}"),
            });
        }
        card.steps.push(Step {
            kind,
            name: words.to_string(),
            start: span.since,
            end,
            host: None,
            detail: String::new(),
            attributed: true,
            leaf: true,
        });
    }
    if let Some(closed) = closed {
        card.moments.push(Moment {
            at: closed.closed.at,
            words: format!("closed, {} by {}", closed.how.words(), closed.closed.by),
        });
    }
    card.moments.sort_by_key(|moment| moment.at);
    card.steps.sort_by_key(|step| (step.start, step.end));
    card.states.sort_by_key(|(at, _)| *at);
    card.attempts.sort_by_key(|attempt| attempt.at);
    Ok(card)
}

/// The first line of <excerpt> that holds more than blanks, trimmed.
fn first_line(excerpt: Option<&str>) -> Option<String> {
    excerpt?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// The `builds cargo` runs of <worktree> that started in [<from>, <to>].
fn read_cargo(db: &Connection, worktree: &str, from: i64, to: i64, card: &mut Card) -> Result<()> {
    let mut statement = db
        .prepare(
            "SELECT r.started_at, r.subcommand, r.host, r.cores, r.load, r.lock_wait_ms, r.ms, \
                    r.hook_started_at IS NOT NULL, \
                    (SELECT group_concat(crate, ',') FROM builds_cargo_run_crate WHERE run_id = r.id), \
                    c.head, c.changes, c.run_branch, c.off_branch, o.outcome, o.excerpt, \
                    (SELECT group_concat(test, ',') FROM builds_cargo_run_failed_test WHERE run_id = r.id) \
             FROM builds_cargo_runs r LEFT JOIN builds_cargo_run_commit c ON c.run_id = r.id \
             LEFT JOIN builds_cargo_run_outcome o ON o.run_id = r.id \
             WHERE r.worktree = ?1 AND r.started_at >= ?2 AND r.started_at <= ?3",
        )
        .sql()?;
    let rows = statement
        .query_map(params![worktree, from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, Option<String>>(8)?,
                (
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<bool>>(12)?,
                ),
                (
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                ),
            ))
        })
        .sql()?;
    for row in rows {
        let (at, subcommand, host, cores, load, lock_wait, ms, in_hook, crates, commit, ended) =
            row.sql()?;
        let name = match subcommand.as_deref() {
            Some(subcommand) => format!("cargo {subcommand}"),
            None => "cargo (subcommand not recorded)".to_string(),
        };
        // The step a later run repeats: the subcommand on the same crates.
        let step = match &crates {
            Some(crates) => format!("{name} -p {crates}"),
            None => name.clone(),
        };
        let mut detail = format!("load {load:.0}/{cores} when it started");
        if let Some(crates) = &crates {
            detail = format!("{crates}; {detail}");
        }
        let (outcome, excerpt, failed) = ended;
        let failed: Vec<String> = failed
            .map(|failed| failed.split(',').map(str::to_string).collect())
            .unwrap_or_default();
        if let Some(outcome) = &outcome {
            let _ = write!(detail, "; {outcome}");
            if !failed.is_empty() {
                let _ = write!(detail, ": {}", failed.join(", "));
            }
            if let Some(first) = first_line(excerpt.as_deref()) {
                let _ = write!(detail, " ({first})");
            }
        }
        if !in_hook
            && let Some(outcome) = outcome.as_deref().filter(|outcome| *outcome != "stopped")
        {
            card.attempts.push(Attempt {
                kind: StepKind::Cargo,
                name: step,
                at,
                result: if outcome == "passed" {
                    RunResult::Passed
                } else {
                    RunResult::Failed(Some(StepFailure::new(excerpt.clone(), failed.clone())))
                },
                state: commit
                    .0
                    .as_ref()
                    .map(|head| format!("{head} {}", commit.1.as_deref().unwrap_or(""))),
            });
        }
        if let (Some(head), changes, run_branch, off) = commit {
            let head = short(&head);
            let _ = write!(detail, "; on {head}");
            if changes.is_some() {
                detail.push_str(" with uncommitted changes");
            }
            match (off, run_branch) {
                (Some(true), Some(run_branch)) => {
                    detail.push_str(&format!(", off branch {run_branch}"));
                    card.off_branch
                        .push((at, format!("{name} on {head}, off branch {run_branch}")));
                }
                (Some(true), None) => {
                    fail!(
                        "the store marks the cargo run at {at} off its run's branch and names no branch"
                    )
                }
                _ => {}
            }
        }
        if let Some(lock_wait) = lock_wait.filter(|wait| *wait > 0) {
            let _ = write!(
                detail,
                "; {} waiting for cargo's locks",
                seconds(lock_wait as u64)
            );
        }
        if in_hook {
            detail.push_str("; started by a hook check");
        }
        card.steps.push(Step {
            kind: Kind::Cargo,
            name,
            start: at,
            end: at + (ms + 999) / 1000,
            host: Some(host),
            detail,
            // A hook check's cargo run is inside the hook run.
            attributed: !in_hook,
            leaf: !in_hook,
        });
    }
    Ok(())
}

/// The commit hook runs of <worktree>, whose run's branch is <run_branch>,
/// that started in [<from>, <to>], with the time of each check that passed. A
/// hook run on another branch or a detached HEAD ran off the run's branch.
fn read_hooks(
    db: &Connection,
    worktree: &str,
    run_branch: &str,
    from: i64,
    to: i64,
    card: &mut Card,
) -> Result<()> {
    let mut runs = db
        .prepare(
            "SELECT id, started_at, plan_ms, total_ms, passed, branch, base FROM hook_runs \
             WHERE worktree = ?1 AND started_at >= ?2 AND started_at <= ?3",
        )
        .sql()?;
    let rows = runs
        .query_map(params![worktree, from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .sql()?;
    for row in rows {
        let (id, at, plan_ms, total_ms, passed, branch, base) = row.sql()?;
        if branch.as_deref() != Some(run_branch) {
            let on = match &branch {
                Some(branch) => format!("branch {branch}"),
                None => "a detached HEAD".to_string(),
            };
            let base = base
                .as_deref()
                .map_or("an unread commit".to_string(), |base| {
                    short(base).to_string()
                });
            card.off_branch.push((
                at,
                format!("commit hook on {base} of {on}, off branch {run_branch}"),
            ));
        }
        let local = Some(LOCAL.to_string());
        let step = |kind, name: String, start, ms: i64, detail: String, attributed, leaf| Step {
            kind,
            name,
            start,
            end: start + (ms + 999) / 1000,
            host: local.clone(),
            detail,
            attributed,
            leaf,
        };
        card.steps.push(step(
            Kind::Hook,
            "commit hook".to_string(),
            at,
            total_ms,
            if passed { "passed" } else { "failed" }.to_string(),
            true,
            false,
        ));
        card.steps.push(step(
            Kind::Hook,
            "hook plan".to_string(),
            at,
            plan_ms,
            String::new(),
            false,
            true,
        ));
        // What the checks ran on: the commit and the staged files.
        let staged: String = db
            .query_row(
                "SELECT ifnull(group_concat(path || ' ' || change || ' ' || ifnull(added, '') \
                        || ' ' || ifnull(removed, ''), ','), '') \
                 FROM (SELECT * FROM hook_run_files WHERE run_id = ?1 ORDER BY path)",
                [id],
                |row| row.get(0),
            )
            .sql()?;
        let state = base.as_ref().map(|base| format!("{base} {staged}"));
        let mut said = crate::hook_check_failure::read(db, id)?;
        let mut outcomes = db
            .prepare(
                "SELECT position, name, outcome = 'passed' FROM hook_run_checks \
                 WHERE run_id = ?1 AND kind = 'check' AND outcome <> 'skipped' ORDER BY position",
            )
            .sql()?;
        let found = outcomes
            .query_map([id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .sql()?;
        for check in found {
            let (position, name, passed) = check.sql()?;
            card.attempts.push(Attempt {
                kind: StepKind::Hook,
                name,
                at,
                result: if passed {
                    RunResult::Passed
                } else {
                    RunResult::Failed(said.remove(&(position as u32)))
                },
                state: state.clone(),
            });
        }
        let mut checks = db
            .prepare(
                "SELECT name, ms FROM hook_run_checks \
                 WHERE run_id = ?1 AND outcome = 'passed' ORDER BY position",
            )
            .sql()?;
        let rows = checks
            .query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .sql()?;
        for check in rows {
            let (name, ms) = check.sql()?;
            card.steps.push(step(
                Kind::Hook,
                format!("hook check {name}"),
                at + plan_ms / 1000,
                ms,
                String::new(),
                false,
                true,
            ));
        }
    }
    Ok(())
}

/// The app builds of <repo> that built <branch> and joined their lane's
/// queue in [<from>, <to>].
fn read_app_builds(
    db: &Connection,
    repo: &str,
    branch: &str,
    from: i64,
    to: i64,
    card: &mut Card,
) -> Result<()> {
    let mut runs = db
        .prepare(
            "SELECT r.id, r.started_at, r.lane, r.checkout, r.wait_ms, r.outcome, c.built_commit \
             FROM app_build_runs r LEFT JOIN app_build_run_commit c ON c.run_id = r.id \
             WHERE r.repo = ?1 AND r.built_ref = ?2 AND r.started_at >= ?3 AND r.started_at <= ?4",
        )
        .sql()?;
    let rows = runs
        .query_map(params![repo, branch, from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .sql()?;
    for row in rows {
        let (id, at, lane, checkout, wait_ms, outcome, built) = row.sql()?;
        let host = host_of_checkout(&checkout);
        let built_words = built
            .as_deref()
            .map_or(String::new(), |sha| format!(" {}", short(sha)));
        card.steps.push(Step {
            kind: Kind::AppBuildQueue,
            name: format!("app build {lane} queue"),
            start: at,
            end: at + (wait_ms + 999) / 1000,
            host: Some(host.clone()),
            detail: format!("then built{built_words} in {checkout}; {outcome}"),
            attributed: true,
            leaf: true,
        });
        let mut said = crate::app_build_step_failure::read(db, id)?;
        let mut steps = db
            .prepare(
                "SELECT position, name, started_at, wall_ms, result FROM app_build_steps \
                 WHERE run_id = ?1 ORDER BY position",
            )
            .sql()?;
        let rows = steps
            .query_map([id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .sql()?;
        for step in rows {
            let (position, name, start, wall_ms, result) = step.sql()?;
            // A step another build took over, or a stop signal ended, says
            // nothing of the change.
            if result != "taken over" && result != "stopped" {
                card.attempts.push(Attempt {
                    kind: StepKind::AppBuild,
                    name: format!("{lane} {name}"),
                    at: start,
                    result: if result == "passed" {
                        RunResult::Passed
                    } else {
                        RunResult::Failed(said.remove(&(position as u32)))
                    },
                    state: built.clone(),
                });
            }
            card.steps.push(Step {
                kind: Kind::AppBuildStep,
                name: format!("app build {lane} {name}"),
                start,
                end: start + (wall_ms + 999) / 1000,
                host: Some(host.clone()),
                detail: result,
                attributed: true,
                leaf: true,
            });
        }
    }
    Ok(())
}

/// The review runs of <repo>'s branch <branch> that started in [<from>,
/// <to>], with each group's time and each settle of the runs' findings.
fn read_reviews(
    db: &Connection,
    repo: &str,
    branch: &str,
    from: i64,
    to: i64,
    card: &mut Card,
) -> Result<()> {
    let end_at = crate::review::exchange_time::END_AT;
    let mut runs = db
        .prepare(&format!(
            "SELECT run.id, run.at, \
                    (SELECT MAX({end_at}) FROM review_exchange_time t \
                       JOIN review_exchange e ON e.id = t.exchange_id \
                       JOIN review_call c ON c.id = e.call_id WHERE c.run_id = run.id), \
                    (SELECT COUNT(*) FROM review_finding f JOIN review_call c ON c.id = f.call_id \
                       WHERE c.run_id = run.id AND f.check_failed IS NULL) \
             FROM review_run run JOIN review_landing l ON l.id = run.landing_id \
             WHERE l.key = ?1 AND l.repo = ?2 AND run.kind = 'review' \
               AND run.at >= ?3 AND run.at <= ?4 ORDER BY run.at, run.id",
        ))
        .sql()?;
    let rows: Vec<(i64, i64, Option<i64>, i64)> = runs
        .query_map(params![format!("branch:{branch}"), repo, from, to], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .sql()?
        .collect::<std::result::Result<_, _>>()
        .sql()?;
    for (id, at, end, findings) in rows {
        let end = end.unwrap_or(at).max(at);
        card.steps.push(Step {
            kind: Kind::Review,
            name: "review run".to_string(),
            start: at,
            end,
            host: None,
            detail: counted(findings as usize, "finding", "findings"),
            attributed: true,
            leaf: false,
        });
        let mut groups = db
            .prepare(&format!(
                "SELECT c.group_name, MIN(t.started_at), MAX({end_at}), \
                        (SELECT COUNT(*) FROM review_finding f WHERE f.call_id = c.id AND f.check_failed IS NULL) \
                 FROM review_call c JOIN review_exchange e ON e.call_id = c.id \
                   JOIN review_exchange_time t ON t.exchange_id = e.id \
                 WHERE c.run_id = ?1 GROUP BY c.id",
            ))
            .sql()?;
        let found_groups = groups
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .sql()?;
        for group in found_groups {
            let (name, start, stop, found) = group.sql()?;
            card.steps.push(Step {
                kind: Kind::Review,
                name: format!("review {name}"),
                start,
                end: stop.max(start),
                host: None,
                detail: counted(found as usize, "finding", "findings"),
                attributed: false,
                leaf: true,
            });
        }
        read_settles(db, id, card)?;
        read_findings(db, id, card)?;
    }
    Ok(())
}

/// The landings that named item <id>: each landing's steps, and the commits
/// it was asked to land.
fn read_landings(db: &Connection, id: Id, card: &mut Card) -> Result<()> {
    let end_at = crate::land_run::END_AT;
    let mut runs = db
        .prepare(&format!(
            "SELECT l.id, l.started_at, {end_at}, l.landed FROM land_run l \
             JOIN land_run_item i ON i.land_id = l.id WHERE i.item_id = ?1 ORDER BY l.started_at",
        ))
        .sql()?;
    let rows: Vec<(i64, i64, i64, bool)> = runs
        .query_map([id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .sql()?
        .collect::<std::result::Result<_, _>>()
        .sql()?;
    for (land, started_at, ended_at, landed) in rows {
        card.steps.push(Step {
            kind: Kind::Landing,
            name: "builds land".to_string(),
            start: started_at,
            end: ended_at,
            host: Some(LOCAL.to_string()),
            detail: if landed { "landed" } else { "refused" }.to_string(),
            attributed: true,
            leaf: false,
        });
        let mut steps = db
            .prepare(
                "SELECT kind, started_at, ms FROM land_step WHERE land_id = ?1 ORDER BY position",
            )
            .sql()?;
        let found = steps
            .query_map([land], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .sql()?;
        for step in found {
            let (kind, start, ms) = step.sql()?;
            card.steps.push(Step {
                kind: Kind::Landing,
                name: format!("land {kind}"),
                start,
                end: start + (ms + 999) / 1000,
                host: Some(LOCAL.to_string()),
                detail: String::new(),
                attributed: false,
                leaf: true,
            });
        }
        let mut commits = db
            .prepare("SELECT sha, committed_at FROM land_run_commit WHERE land_id = ?1 ORDER BY position")
            .sql()?;
        let found: Vec<(String, i64)> = commits
            .query_map([land], |row| Ok((row.get(0)?, row.get(1)?)))
            .sql()?
            .collect::<std::result::Result<_, _>>()
            .sql()?;
        for (sha, at) in &found {
            card.moments.push(Moment {
                at: *at,
                words: format!("commit {}", short(sha)),
            });
        }
    }
    Ok(())
}

/// Each settle of the findings of review run <run>, with who made it: the
/// coordinator's first settle, a second settle as dropped, and the person's
/// allowing a finding to land.
fn read_settles(db: &Connection, run: i64, card: &mut Card) -> Result<()> {
    let mut statement = db
        .prepare(
            "SELECT f.id, 'settled as ' || f.settled, f.settled_at, f.settled_by \
               FROM review_finding f JOIN review_call c ON c.id = f.call_id \
               WHERE c.run_id = ?1 AND f.settled IS NOT NULL \
             UNION ALL SELECT a.finding_id, 'settled again as dropped', a.at, a.by \
               FROM review_settle_again a JOIN review_finding f ON f.id = a.finding_id \
               JOIN review_call c ON c.id = f.call_id WHERE c.run_id = ?1 \
             UNION ALL SELECT w.finding_id, 'allowed to land', w.at, w.by \
               FROM review_allowed w JOIN review_finding f ON f.id = w.finding_id \
               JOIN review_call c ON c.id = f.call_id WHERE c.run_id = ?1",
        )
        .sql()?;
    let rows = statement
        .query_map([run], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .sql()?;
    for row in rows {
        let (finding, how, at, by) = row.sql()?;
        card.moments.push(Moment {
            at,
            words: format!("finding {finding} {how} by {by}"),
        });
    }
    Ok(())
}

/// The findings of review run <run> that builds' own checks kept, with how
/// each counts now and whether a later run closed it.
fn read_findings(db: &Connection, run: i64, card: &mut Card) -> Result<()> {
    let mut statement = db
        .prepare(
            "SELECT c.group_name, \
                    CASE WHEN a.finding_id IS NOT NULL THEN 'dropped' ELSE f.settled END, \
                    z.finding_id IS NOT NULL \
             FROM review_finding f JOIN review_call c ON c.id = f.call_id \
             LEFT JOIN review_settle_again a ON a.finding_id = f.id \
             LEFT JOIN review_closed z ON z.finding_id = f.id \
             WHERE c.run_id = ?1 AND f.check_failed IS NULL",
        )
        .sql()?;
    let rows = statement
        .query_map([run], |row| {
            Ok(Finding {
                group: row.get(0)?,
                settled: row.get(1)?,
                closed: row.get(2)?,
            })
        })
        .sql()?;
    for finding in rows {
        card.findings.push(finding.sql()?);
    }
    Ok(())
}

/// The stretches of dashboard state of the cards that listed item <id>, cut
/// to the card's time, each as a step; the review's and the landing's waits
/// count toward their kinds (`dashboard_kind`).
fn read_dashboard(db: &Connection, id: Id, card: &mut Card) -> Result<()> {
    for stretch in crate::card_state::read_item(db, id, card.to)? {
        let start = stretch.start.max(card.from);
        let end = stretch.end.min(card.to);
        if end < start {
            continue;
        }
        let kind = dashboard_kind(&stretch.state);
        card.steps.push(Step {
            kind: kind.unwrap_or(Kind::Working),
            name: format!("dashboard {}", stretch.state),
            start,
            end,
            host: None,
            detail: crate::checkout::file_name(std::path::Path::new(&stretch.worktree)),
            attributed: kind.is_some(),
            leaf: false,
        });
        card.dashboard.push(crate::card_state::Stretch {
            start,
            end,
            ..stretch
        });
    }
    Ok(())
}

/// The kind a stretch of dashboard state <state> counts toward; None for a
/// state the card's runs, spans and steps already account for.
fn dashboard_kind(state: &str) -> Option<Kind> {
    match state {
        "reviewing" => Some(Kind::Review),
        "findings-to-settle" => Some(Kind::WaitingForSettle),
        "open-findings" => Some(Kind::Fixing),
        "needs-review" | "unreviewed-changes" => Some(Kind::WaitingForReview),
        "ready-to-land" => Some(Kind::WaitingToLand),
        _ => None,
    }
}

/// The time <card>'s cards spent in each dashboard state.
pub fn dashboard_seconds(card: &Card) -> BTreeMap<&str, i64> {
    let mut seconds: BTreeMap<&str, i64> = BTreeMap::new();
    for stretch in &card.dashboard {
        *seconds.entry(stretch.state.as_str()).or_default() += stretch.end - stretch.start;
    }
    seconds
}

/// What a host did over [<from>, <to>], in words; None when it has no sample
/// there.
fn host_words(db: &Connection, host: &str, from: i64, to: i64) -> Result<Option<String>> {
    db.query_row(
        "SELECT COUNT(*), AVG(load_1), MAX(load_1), MAX(cores), \
                MAX(CASE memory WHEN 'critical' THEN 2 WHEN 'warning' THEN 1 ELSE 0 END) \
         FROM host_sample WHERE host = ?1 AND at >= ?2 AND at <= ?3",
        params![host, from, to],
        |row| {
            let samples: i64 = row.get(0)?;
            if samples == 0 {
                return Ok(None);
            }
            let (average, peak, cores, memory): (f64, f64, i64, i64) =
                (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?);
            Ok(Some(format!(
                "{host} load {average:.0} on average and {peak:.0} at most of {cores} cores, memory {} ({})",
                ["normal", "warning", "critical"][memory as usize],
                counted(samples as usize, "sample", "samples")
            )))
        },
    )
    .sql()
}

/// How many seconds of the card's time each kind holds: every moment from
/// `from` to `to` goes to the highest kind of the steps that cover it, else
/// to the agent's run, else to the state the item was in.
pub fn attribute(card: &Card) -> BTreeMap<Kind, i64> {
    let mut points: BTreeSet<i64> = BTreeSet::new();
    points.insert(card.from);
    points.insert(card.to);
    let clip = |at: i64| at.clamp(card.from, card.to);
    for step in card.steps.iter().filter(|step| step.attributed) {
        points.insert(clip(step.start));
        points.insert(clip(step.end));
    }
    for (start, end) in &card.runs {
        points.insert(clip(*start));
        points.insert(clip(*end));
    }
    for (at, _) in &card.states {
        points.insert(clip(*at));
    }
    let points: Vec<i64> = points.into_iter().collect();
    let first_run = card.runs.iter().map(|(start, _)| *start).min();
    let mut kinds: BTreeMap<Kind, i64> = BTreeMap::new();
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let covering = card
            .steps
            .iter()
            .filter(|step| {
                step.attributed && step.start <= a && step.end >= b && step.end > step.start
            })
            .map(|step| step.kind)
            .min();
        let kind = covering.unwrap_or_else(|| {
            if card
                .runs
                .iter()
                .any(|(start, end)| *start <= a && *end >= b)
            {
                Kind::Working
            } else if first_run.is_some_and(|first| a >= first) {
                Kind::BetweenRuns
            } else {
                card.states
                    .iter()
                    .rev()
                    .find(|(at, _)| *at <= a)
                    .map_or(Kind::NotRecorded, |(_, kind)| *kind)
            }
        });
        *kinds.entry(kind).or_default() += b - a;
    }
    kinds
}

/// The timeline of <card>: its moments and steps in order, each step with
/// its host's load, then the time by kind.
pub fn card_text(db: &Connection, card: &Card) -> Result<String> {
    enum Line<'a> {
        Moment(&'a Moment),
        Step(&'a Step),
    }
    let mut lines: Vec<(i64, u8, Line)> = card
        .moments
        .iter()
        .map(|moment| (moment.at, 0, Line::Moment(moment)))
        .chain(
            card.steps
                .iter()
                .map(|step| (step.start, 1, Line::Step(step))),
        )
        .collect();
    lines.sort_by_key(|(at, order, _)| (*at, *order));
    let mut out = String::new();
    let _ = writeln!(out, "timeline of {} {}", card.id, card.title);
    for (_, _, line) in lines {
        match line {
            Line::Moment(moment) => {
                let _ = writeln!(out, "  {}  {}", local_second(moment.at), moment.words);
            }
            Line::Step(step) => {
                let nested = if step.attributed { "" } else { "  " };
                let _ = write!(
                    out,
                    "  {}  {nested}{} took {}",
                    local_second(step.start),
                    step.name,
                    seconds(((step.end - step.start) * 1000).max(0) as u64)
                );
                if !step.detail.is_empty() {
                    let _ = write!(out, " ({})", step.detail);
                }
                if let Some(host) = &step.host {
                    match host_words(db, host, step.start, step.end)? {
                        Some(words) => {
                            let _ = write!(out, "; on {words}");
                        }
                        None => {
                            let _ = write!(out, "; on {host}, which has no sample over it");
                        }
                    }
                }
                out.push('\n');
            }
        }
    }
    let kinds = attribute(card);
    let total: i64 = kinds.values().sum();
    let _ = writeln!(
        out,
        "time by kind, {} from {} to {}",
        duration(total),
        local_second(card.from),
        local_second(card.to)
    );
    let mut by_time: Vec<(&Kind, &i64)> =
        kinds.iter().filter(|(_, seconds)| **seconds > 0).collect();
    by_time.sort_by_key(|(_, seconds)| std::cmp::Reverse(**seconds));
    for (kind, seconds) in by_time {
        let _ = writeln!(
            out,
            "  {:>8}  {:>3}%  {}",
            duration_fine(*seconds),
            *seconds * 100 / total.max(1),
            kind.words()
        );
    }
    out.push_str(&states_text(card, total));
    let failures = crate::step_outcomes::failure_outcomes(&card.attempts);
    if !failures.is_empty() {
        let _ = writeln!(out, "runs that failed, and what followed on the card");
        for (run, outcome) in failures {
            let mut what = format!("{} {}", run.kind.words(), run.name);
            let failed = run
                .result
                .failure()
                .map_or(&[][..], |said| &said.failed[..]);
            if !failed.is_empty() {
                let _ = write!(what, " ({})", failed.join(", "));
            }
            let followed = match outcome {
                crate::step_outcomes::Outcome::Fixed => "a later run passed after a fix",
                crate::step_outcomes::Outcome::Flaky => {
                    "a later run passed on the same commit and changes"
                }
                crate::step_outcomes::Outcome::Stood => "no later run passed",
            };
            let _ = writeln!(out, "  {}  {what}: {followed}", local_second(run.at));
            match &run.result {
                RunResult::Failed(None) => {
                    let _ = writeln!(out, "      no output kept");
                }
                RunResult::Failed(Some(said)) => {
                    if let Some(cause) = first_line(said.excerpt.as_deref()) {
                        let _ = writeln!(out, "      {cause}");
                    }
                }
                RunResult::Passed => {}
            }
        }
    }
    if !card.off_branch.is_empty() {
        let _ = writeln!(
            out,
            "steps that ran off their run's branch (a bisect or an old checkout)"
        );
        for (at, words) in &card.off_branch {
            let _ = writeln!(out, "  {}  {words}", local_second(*at));
        }
    }
    Ok(out)
}

/// The time <card>'s cards spent in each dashboard state, the longest first,
/// each with its share of <total>, the card's time.
fn states_text(card: &Card, total: i64) -> String {
    let mut out = String::new();
    let states = dashboard_seconds(card);
    if states.is_empty() {
        let _ = writeln!(
            out,
            "no dashboard state recorded for the cards that listed it"
        );
        return out;
    }
    let _ = writeln!(out, "time in each dashboard state");
    let mut by_time: Vec<(&str, i64)> = states.into_iter().collect();
    by_time.sort_by_key(|(_, seconds)| std::cmp::Reverse(*seconds));
    for (state, seconds) in by_time {
        let _ = writeln!(
            out,
            "  {:>8}  {:>3}%  {state}",
            duration_fine(seconds),
            seconds * 100 / total.max(1),
        );
    }
    out
}

/// <seconds> as `6d2h`, `3h05m`, `12m` or, under a minute, `40s`.
fn duration_fine(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        duration(seconds)
    }
}

/// The value at fraction <p> of <sorted>, which must not be empty.
fn percentile(sorted: &[i64], p: f64) -> i64 {
    let at = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[at]
}

/// The totals over <cards>: the time by kind, then the slowest steps.
pub fn totals_text(cards: &[Card]) -> String {
    let mut out = String::new();
    if cards.is_empty() {
        let _ = writeln!(out, "no card landed in the last {RECENT_DAYS} days");
        return out;
    }
    let mut by_kind: BTreeMap<Kind, Vec<i64>> = BTreeMap::new();
    let mut every: i64 = 0;
    let mut by_step: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for card in cards {
        for (kind, seconds) in attribute(card) {
            if seconds > 0 {
                by_kind.entry(kind).or_default().push(seconds);
                every += seconds;
            }
        }
        for step in card
            .steps
            .iter()
            .filter(|step| step.leaf && step.end > step.start)
        {
            by_step
                .entry(step.name.clone())
                .or_default()
                .push(step.end - step.start);
        }
    }
    let _ = writeln!(
        out,
        "{} that landed in the last {RECENT_DAYS} days, {} in all",
        counted(cards.len(), "card", "cards"),
        duration(every)
    );
    let _ = writeln!(
        out,
        "time by kind (cards with the kind; median and 90th percentile among them)"
    );
    let mut kinds: Vec<(Kind, Vec<i64>)> = by_kind.into_iter().collect();
    kinds.sort_by_key(|(_, times)| std::cmp::Reverse(times.iter().sum::<i64>()));
    for (kind, mut times) in kinds {
        times.sort_unstable();
        let _ = writeln!(
            out,
            "  {:>3}%  median {:>6}  p90 {:>6}  {:>4} cards  {}",
            times.iter().sum::<i64>() * 100 / every.max(1),
            duration_fine(percentile(&times, 0.5)),
            duration_fine(percentile(&times, 0.9)),
            times.len(),
            kind.words()
        );
    }
    let _ = writeln!(
        out,
        "slowest steps (all their runs; the share counts a step against all the cards' time)"
    );
    let mut steps: Vec<(String, Vec<i64>)> = by_step.into_iter().collect();
    steps.sort_by_key(|(_, times)| std::cmp::Reverse(times.iter().sum::<i64>()));
    for (name, mut times) in steps.into_iter().take(20) {
        times.sort_unstable();
        let _ = writeln!(
            out,
            "  {:>3}%  median {:>6}  p90 {:>6}  {:>5} runs  {name}",
            times.iter().sum::<i64>() * 100 / every.max(1),
            duration_fine(percentile(&times, 0.5)),
            duration_fine(percentile(&times, 0.9)),
            times.len(),
        );
    }
    let mut by_state: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for card in cards {
        for (state, seconds) in dashboard_seconds(card) {
            by_state.entry(state).or_default().push(seconds);
        }
    }
    let _ = writeln!(
        out,
        "time in each dashboard state (cards with the state; median and 90th percentile among them)"
    );
    let mut states: Vec<(&str, Vec<i64>)> = by_state.into_iter().collect();
    states.sort_by_key(|(_, times)| std::cmp::Reverse(times.iter().sum::<i64>()));
    for (state, mut times) in states {
        times.sort_unstable();
        let _ = writeln!(
            out,
            "  {:>3}%  median {:>6}  p90 {:>6}  {:>4} cards  {state}",
            times.iter().sum::<i64>() * 100 / every.max(1),
            duration_fine(percentile(&times, 0.5)),
            duration_fine(percentile(&times, 0.9)),
            times.len(),
        );
    }
    let caught: Vec<(&[Attempt], &[Finding])> = cards
        .iter()
        .map(|card| (card.attempts.as_slice(), card.findings.as_slice()))
        .collect();
    out.push_str(&crate::step_outcomes::text(&caught));
    out
}

/// What `builds item times` prints beyond the lines `times_text` gives: for
/// each of <ids>, the timeline; with none, the totals over the cards that
/// landed in the last `RECENT_DAYS` days.
pub fn text(db: &Connection, lists: &Lists, ids: &[Id], now: i64) -> Result<String> {
    if !ids.is_empty() {
        let mut out = String::new();
        for id in ids {
            let card = gather(db, lists, *id, now)?;
            out.push_str(&card_text(db, &card)?);
        }
        return Ok(out);
    }
    let recent = now - RECENT_DAYS * 86400;
    let mut cards = Vec::new();
    for closed in &lists.closed {
        if let Record::Item(item) = &closed.record
            && matches!(closed.how, crate::lists::How::Landed(_))
            && closed.closed.at >= recent
        {
            cards.push(gather(db, lists, item.id, now)?);
        }
    }
    Ok(totals_text(&cards))
}

#[cfg(test)]
#[path = "card_profile_tests.rs"]
mod tests;
