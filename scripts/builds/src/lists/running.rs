//! Where an item runs, the runs of it that ended, and the spans of each run
//! during which the worktree's claim note said held or the item waited on
//! the person. The agent is the claim of
//! the item's worktree, never stored on the item, so the two can't disagree;
//! `agent_of` reads the claim. builds refuses to release a worktree while an
//! item runs in it (kept.rs). `findings` is what `builds health` says about
//! the lists.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::health::Advice;
use crate::lists::derive::wait_on_person;
use crate::lists::item::{self, Item};
use crate::lists::store::Loaded;
use crate::lists::{Id, Lists, Stamp};
use crate::places::Places;

/// An item an agent is working on, in one worktree of one repo (an item that
/// spans repos has one of these per repo). The agent is the worktree's claim, so the
/// claim and this marker can't disagree; the repo is the worktree's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Running {
    /// The absolute path of a side worktree of a project repo.
    pub worktree: String,
    pub branch: String,
    pub brief: String,
    pub since: Stamp,
}

/// How a run ended: someone stopped the item or closed it (the
/// item's closing says how).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ended {
    Stopped,
    Closed,
}

impl Ended {
    /// The word the database holds.
    fn word(self) -> &'static str {
        match self {
            Ended::Stopped => "stopped",
            Ended::Closed => "closed",
        }
    }

    fn from_word(word: &str) -> Ended {
        match word {
            "stopped" => Ended::Stopped,
            "closed" => Ended::Closed,
            other => unreachable!("item_run_how_known holds, but the database has {other:?}"),
        }
    }
}

/// A run of an item that ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub since: Stamp,
    pub ended: Stamp,
    pub how: Ended,
    /// The worktree and branch of each repo it ran in. A run that a store
    /// move read from the write log has none.
    pub places: Vec<RunPlace>,
}

/// The worktree and branch an ended run ran in, in one repo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPlace {
    pub repo: String,
    pub worktree: String,
    pub branch: String,
}

/// What a span of a run is: the worktree's claim note said held, or the
/// item waited on a question or on a design change the person had not
/// approved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanKind {
    Held,
    Waiting,
}

impl SpanKind {
    /// The word the database holds.
    pub fn word(self) -> &'static str {
        match self {
            SpanKind::Held => "held",
            SpanKind::Waiting => "waiting",
        }
    }

    fn from_word(word: &str) -> SpanKind {
        match word {
            "held" => SpanKind::Held,
            "waiting" => SpanKind::Waiting,
            other => unreachable!("the span kind CHECKs hold, but the database has {other:?}"),
        }
    }
}

/// A span of a run, from <since> to <ended>, which is None while the span
/// goes on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub kind: SpanKind,
    pub since: i64,
    pub ended: Option<i64>,
}

/// The parent folder, repo and checkout name of a side worktree path,
/// `<parent>/<repo>-worktrees/side-N`, by its shape alone. Whether the folder
/// is a live worktree is asked where a command takes the item.
pub fn side_worktree_split(path: &str) -> Option<(String, String, String)> {
    let path = std::path::Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let checkout = path.file_name()?.to_str()?;
    let digits = checkout.strip_prefix("side-")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let folder = path.parent()?.file_name()?.to_str()?;
    let repo = folder.strip_suffix("-worktrees")?;
    if repo.is_empty() {
        return None;
    }
    let parent = path.parent()?.parent()?.to_str()?;
    Some((parent.to_string(), repo.to_string(), checkout.to_string()))
}

/// `side_worktree_split` of <path>, or a refusal that names the shape a
/// side worktree path has.
pub fn require_side_worktree(path: &str) -> Result<(String, String, String)> {
    match side_worktree_split(path) {
        Some(parts) => Ok(parts),
        None => fail!("{path} is not a side worktree (<parent>/<repo>-worktrees/side-N)"),
    }
}

/// The table of the worktrees an item holds: where it runs, or where it
/// keeps its unfinished work (lists/kept_work.rs). Both have one row per
/// worktree, with its repo, parent folder, checkout name and branch.
pub enum PlaceTable {
    Running,
    Kept,
}

impl PlaceTable {
    fn name(&self) -> &'static str {
        match self {
            PlaceTable::Running => "running_place",
            PlaceTable::Kept => "kept_work_place",
        }
    }
}

/// Writes <places>, each a side worktree and its branch, as item <id>'s rows
/// of <table>.
pub fn insert_places<'a>(
    db: &Connection,
    table: PlaceTable,
    id: Id,
    places: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<()> {
    for (worktree, branch) in places {
        let (parent, repo, checkout) = require_side_worktree(worktree)?;
        db.execute(
            &format!(
                "INSERT INTO {} (item_id, repo, parent, checkout, branch) VALUES (?1, ?2, ?3, ?4, ?5)",
                table.name()
            ),
            params![id, repo, parent, checkout, branch],
        )
        .sql()?;
    }
    Ok(())
}

/// The place of <running> in <repo>, the worktree an item runs in there, if
/// it runs there: an item runs in one worktree per repo.
pub fn place_in_repo<'a>(running: &'a [Running], repo: &str) -> Option<&'a Running> {
    running
        .iter()
        .find(|r| side_worktree_parts(&r.worktree).is_some_and(|(of, _)| of == repo))
}

/// The repo and checkout name of a side worktree path.
pub fn side_worktree_parts(path: &str) -> Option<(String, String)> {
    side_worktree_split(path).map(|(_, repo, checkout)| (repo, checkout))
}

/// Starts open items <ids> running in <places>: the same places, brief and
/// moment for each, which is what the first of <places> says.
pub fn start(db: &Connection, ids: &[Id], places: &[Running]) -> Result<()> {
    let Some(first) = places.first() else {
        fail!("an item runs in at least one worktree")
    };
    for id in ids {
        item::require_open_item(db, *id)?;
        db.execute(
            "INSERT INTO running (item_id, brief, since_at, since_by) VALUES (?1, ?2, ?3, ?4)",
            params![id, first.brief, first.since.at, first.since.by],
        )
        .sql()?;
        insert_places(
            db,
            PlaceTable::Running,
            *id,
            places
                .iter()
                .map(|place| (place.worktree.as_str(), place.branch.as_str())),
        )?;
    }
    Ok(())
}

/// Gives every place open item <id> runs in <brief>. Refused when it does
/// not run.
pub fn handoff(db: &Connection, id: Id, brief: &str) -> Result<()> {
    item::require_open_item(db, id)?;
    let changed = db
        .execute(
            "UPDATE running SET brief = ?2 WHERE item_id = ?1",
            params![id, brief],
        )
        .sql()?;
    if changed == 0 {
        fail!(
            "{id} is not running; a handoff gives a running item's next agent its brief (`item take` starts one)"
        );
    }
    Ok(())
}

/// Ends open item <id>'s running, everywhere, as stopped by <stamp>. stop
/// refuses an item that does not run.
pub fn stop(db: &Connection, id: Id, stamp: &Stamp) -> Result<()> {
    item::require_open_item(db, id)?;
    if end(db, id, Ended::Stopped, stamp)? == 0 {
        fail!("{id} is not running");
    }
    Ok(())
}

/// Ends item <id>'s running wherever it runs, if it runs, as <how> says and
/// at <stamp>: the run, its places and its open spans move to the tables of
/// ended runs and spans. Says how many items it ended (0 or 1): an item that
/// is done runs nowhere.
pub fn end(db: &Connection, id: Id, how: Ended, stamp: &Stamp) -> Result<usize> {
    let ended = db
        .execute(
            "INSERT INTO item_run (item_id, since_at, since_by, how, ended_at, ended_by) \
             SELECT item_id, since_at, since_by, ?2, ?3, ?4 FROM running WHERE item_id = ?1",
            params![id, how.word(), stamp.at, stamp.by],
        )
        .sql()?;
    if ended == 0 {
        return Ok(0);
    }
    db.execute(
        "INSERT INTO item_run_place (run_id, repo, worktree, branch) \
         SELECT ?1, repo, worktree, branch FROM running_place WHERE item_id = ?2",
        params![db.last_insert_rowid(), id],
    )
    .sql()?;
    db.execute(
        "INSERT INTO item_span (item_id, kind, since_at, ended_at) \
         SELECT item_id, kind, since_at, ?2 FROM running_span WHERE item_id = ?1",
        params![id, stamp.at],
    )
    .sql()?;
    db.execute("DELETE FROM running_span WHERE item_id = ?1", [id])
        .sql()?;
    db.execute("DELETE FROM running WHERE item_id = ?1", [id])
        .sql()
}

/// Opens a span of <kind> on running item <id> at <at>, unless one is open:
/// the open one keeps its start. The database refuses the span when the
/// item does not run.
pub fn open_span(db: &Connection, id: Id, kind: SpanKind, at: i64) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO running_span (item_id, kind, since_at) VALUES (?1, ?2, ?3)",
        params![id, kind.word(), at],
    )
    .sql()?;
    Ok(())
}

/// Ends item <id>'s open span of <kind> at <at>, if one is open.
pub fn end_span(db: &Connection, id: Id, kind: SpanKind, at: i64) -> Result<()> {
    db.execute(
        "INSERT INTO item_span (item_id, kind, since_at, ended_at) \
         SELECT item_id, kind, since_at, ?3 FROM running_span WHERE item_id = ?1 AND kind = ?2",
        params![id, kind.word(), at],
    )
    .sql()?;
    db.execute(
        "DELETE FROM running_span WHERE item_id = ?1 AND kind = ?2",
        params![id, kind.word()],
    )
    .sql()?;
    Ok(())
}

/// Opens at <at> a `waiting` span on each running item of <lists> that waits
/// on the person (`derive::wait_on_person`) and has none open, and ends the
/// open one of each running item that no longer waits on the person. Every
/// write runs this on the state it leaves (store.rs), so the spans start and
/// end with the writes that change what an item waits on.
pub fn follow_waits(db: &Connection, lists: &Lists, at: i64) -> Result<()> {
    for item in lists.items.iter().filter(|item| !item.running.is_empty()) {
        let waits = item.waits.iter().any(|wait| wait_on_person(lists, wait));
        let open = item
            .spans
            .iter()
            .any(|span| span.kind == SpanKind::Waiting && span.ended.is_none());
        match (waits, open) {
            (true, false) => open_span(db, item.id, SpanKind::Waiting, at)?,
            (false, true) => end_span(db, item.id, SpanKind::Waiting, at)?,
            _ => {}
        }
    }
    Ok(())
}

/// Where every running item runs, by item.
pub fn read(db: &Connection) -> Result<BTreeMap<Id, Vec<Running>>> {
    let mut statement = db
        .prepare(
            "SELECT r.item_id, p.worktree, p.branch, r.brief, r.since_at, r.since_by \
             FROM running r JOIN running_place p ON p.item_id = r.item_id ORDER BY r.item_id, p.repo",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                Running {
                    worktree: row.get(1)?,
                    branch: row.get(2)?,
                    brief: row.get(3)?,
                    since: Stamp {
                        at: row.get(4)?,
                        by: row.get(5)?,
                    },
                },
            ))
        })
        .sql()?;
    let mut all: BTreeMap<Id, Vec<Running>> = BTreeMap::new();
    for row in rows {
        let (id, running) = row.sql()?;
        all.entry(id).or_default().push(running);
    }
    Ok(all)
}

/// Every ended run, by item, oldest first.
pub fn read_runs(db: &Connection) -> Result<BTreeMap<Id, Vec<Run>>> {
    let mut places: BTreeMap<i64, Vec<RunPlace>> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT run_id, repo, worktree, branch FROM item_run_place ORDER BY run_id, repo")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                RunPlace {
                    repo: row.get(1)?,
                    worktree: row.get(2)?,
                    branch: row.get(3)?,
                },
            ))
        })
        .sql()?
    {
        let (run, place) = row.sql()?;
        places.entry(run).or_default().push(place);
    }
    let mut statement = db
        .prepare(
            "SELECT id, item_id, since_at, since_by, how, ended_at, ended_by FROM item_run \
             ORDER BY item_id, since_at, id",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Id>(1)?,
                Stamp {
                    at: row.get(2)?,
                    by: row.get(3)?,
                },
                row.get::<_, String>(4)?,
                Stamp {
                    at: row.get(5)?,
                    by: row.get(6)?,
                },
            ))
        })
        .sql()?;
    let mut all: BTreeMap<Id, Vec<Run>> = BTreeMap::new();
    for row in rows {
        let (run, id, since, how, ended) = row.sql()?;
        all.entry(id).or_default().push(Run {
            since,
            ended,
            how: Ended::from_word(&how),
            places: places.remove(&run).unwrap_or_default(),
        });
    }
    Ok(all)
}

/// Every span, by item: the ended ones oldest first, then the open ones.
pub fn read_spans(db: &Connection) -> Result<BTreeMap<Id, Vec<Span>>> {
    let mut statement = db
        .prepare(
            "SELECT item_id, kind, since_at, ended_at FROM \
             (SELECT item_id, kind, since_at, ended_at, 0 AS open, id FROM item_span \
              UNION ALL SELECT item_id, kind, since_at, NULL, 1, 0 FROM running_span) \
             ORDER BY item_id, open, since_at, id, kind",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                Span {
                    kind: SpanKind::from_word(&row.get::<_, String>(1)?),
                    since: row.get(2)?,
                    ended: row.get(3)?,
                },
            ))
        })
        .sql()?;
    let mut all: BTreeMap<Id, Vec<Span>> = BTreeMap::new();
    for row in rows {
        let (id, span) = row.sql()?;
        all.entry(id).or_default().push(span);
    }
    Ok(all)
}

/// When the store began to record spans, or None when the store has
/// recorded spans since builds made it (running.sql's `spans_recorded`).
pub fn read_spans_from(db: &Connection) -> Result<Option<i64>> {
    db.query_row("SELECT from_at FROM spans_recorded", [], |row| row.get(0))
        .optional()
        .sql()
}

/// Who holds the worktree an item runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Agent {
    Claimed(String),
    Unclaimed,
    WorktreeGone,
}

impl Agent {
    pub fn words(&self) -> String {
        match self {
            Agent::Claimed(who) => who.clone(),
            Agent::Unclaimed => "no claim".to_string(),
            Agent::WorktreeGone => "worktree gone".to_string(),
        }
    }
}

pub fn agent_of(running: &Running) -> Result<Agent> {
    let path = Path::new(&running.worktree);
    if !path.is_dir() {
        return Ok(Agent::WorktreeGone);
    }
    agent_in(Checkout::at(path)?.as_ref())
}

/// The worktree at <path> when git says <path> is the top of a checkout; None
/// when the folder is gone, is not a checkout, or is only inside one. Liveness
/// is git's word, whatever builds output the repo has.
pub fn live_worktree(path: &str) -> Result<Option<Checkout>> {
    let path = Path::new(path);
    let Ok(wanted) = path.canonicalize() else {
        return Ok(None);
    };
    Ok(Checkout::at(&wanted)?.filter(|checkout| checkout.top == wanted))
}

/// The agent of an item running in <checkout> (None: no live worktree there).
fn agent_in(checkout: Option<&Checkout>) -> Result<Agent> {
    let Some(checkout) = checkout else {
        return Ok(Agent::WorktreeGone);
    };
    Ok(match Claim::read(&checkout.claim_file())? {
        Some(claim) if !claim.who.is_empty() => Agent::Claimed(claim.who),
        _ => Agent::Unclaimed,
    })
}

/// The line that says what runs where, the one to read after a compact:
/// `Running: <agent>, <repo> <side-N> <branch>, brief <path>`.
pub fn marker(running: &Running) -> Result<String> {
    let (repo, checkout) = side_worktree_parts(&running.worktree)
        .unwrap_or_else(|| ("?".to_string(), running.worktree.clone()));
    Ok(format!(
        "Running: {}, {repo} {checkout} {}, brief {}",
        agent_of(running)?.words(),
        running.branch,
        running.brief
    ))
}

/// The open items running in <worktree>.
pub fn running_in<'a>(lists: &'a Lists, worktree: &str) -> Vec<&'a Item> {
    lists
        .items
        .iter()
        .filter(|item| item.running.iter().any(|r| r.worktree == worktree))
        .collect()
}

/// The open items of every one of <lists> (each project's) that run in
/// <worktree>, each with its lists: in a repo two projects hold, an item of
/// either project may run in any of its worktrees, whatever project the
/// worktree's claim names.
pub fn running_in_any<'a>(lists: &'a [Lists], worktree: &str) -> Vec<(&'a Lists, &'a Item)> {
    lists
        .iter()
        .flat_map(|lists| {
            running_in(lists, worktree)
                .into_iter()
                .map(move |item| (lists, item))
        })
        .collect()
}

/// What `builds health` says about the lists: a store whose database fails
/// SQLite's integrity checks, a store that fails its check,
/// and a running item whose worktree is gone or has no claim. `builds
/// release` and a landing's release (kept.rs), `builds
/// item drop` each refuse to remove a claim while another item runs in the
/// worktree, so only a hand edit can leave a running item with no claim;
/// `builds health` reports such an item and does not repair it. One line
/// per fault.
pub fn findings(
    places: &Places,
    loaded: &std::result::Result<Vec<Loaded>, String>,
) -> Result<Vec<Advice>> {
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(fault) => return Ok(vec![Advice::said(format!("lists: {fault}"))]),
    };
    let mut findings: Vec<Advice> = crate::lists::store::integrity(places)
        .into_iter()
        .map(Advice::said)
        .collect();
    for project in loaded {
        let lists = match &project.lists {
            Ok(lists) => lists,
            Err(fault) => {
                findings.push(Advice::said(format!(
                    "lists: project {} fails its check: {fault}",
                    project.project
                )));
                continue;
            }
        };
        for item in &lists.items {
            for running in &item.running {
                let live = live_worktree(&running.worktree)?;
                let fault = match agent_in(live.as_ref())? {
                    Agent::WorktreeGone => "is not a live worktree",
                    Agent::Unclaimed => "has no claim",
                    Agent::Claimed(_) => continue,
                };
                findings.push(Advice::run(
                    format!(
                        "lists: item {} \"{}\" runs in {}, which {fault}; end it with",
                        item.id.0, item.title, running.worktree
                    ),
                    format!("builds item stop {}", item.id.0),
                ));
            }
        }
    }
    Ok(findings)
}

/// The tables running.rs owns.
#[cfg(test)]
const TABLES: [&str; 7] = [
    "running",
    "running_place",
    "item_run",
    "item_run_place",
    "running_span",
    "item_span",
    "spans_recorded",
];

/// Writes a row of one of running.rs's tables as given, the constraints
/// still holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "running_tests.rs"]
mod tests;
