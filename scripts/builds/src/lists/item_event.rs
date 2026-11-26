//! The moments an item changed state or gained or lost a wait
//! (item_event.sql). `follow` records them at the end of every write of the
//! store, from the state the write left: an item's state and each wait that
//! is not met are compared with what the item's last events say, and a
//! difference becomes an event at the write's time. `builds item times` reads
//! them back with the item's runs and spans.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::derive::{ItemState, item_state, unmet_waits, waits_for_others};
use crate::lists::{Id, Lists, Wait};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An agent can start the item.
    Ready,
    /// An unmet wait, or being placed last, keeps the item from starting.
    Blocked,
    /// An agent took the item.
    Running,
    /// The item waits until the person starts it; the detail is why.
    Backlog,
    /// An unmet wait began.
    WaitGained,
    /// An unmet wait ended, because the item stopped waiting on it or the
    /// record it waits on closed.
    WaitLost,
    /// The item's run went to another agent's brief.
    HandedOff,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Ready => "ready",
            Kind::Blocked => "blocked",
            Kind::Running => "running",
            Kind::Backlog => "backlog",
            Kind::WaitGained => "wait-gained",
            Kind::WaitLost => "wait-lost",
            Kind::HandedOff => "handed-off",
        }
    }

    fn from_word(word: &str) -> Result<Kind> {
        Ok(match word {
            "ready" => Kind::Ready,
            "blocked" => Kind::Blocked,
            "running" => Kind::Running,
            "backlog" => Kind::Backlog,
            "wait-gained" => Kind::WaitGained,
            "wait-lost" => Kind::WaitLost,
            "handed-off" => Kind::HandedOff,
            other => fail!("the store holds an item event of kind {other:?}"),
        })
    }

    /// Does the kind say what state the item is in?
    fn is_state(self) -> bool {
        matches!(
            self,
            Kind::Ready | Kind::Blocked | Kind::Running | Kind::Backlog
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub at: i64,
    pub kind: Kind,
    /// The wait of a wait event, the brief of a handoff, or why a blocked
    /// item cannot start when no wait says so.
    pub detail: Option<String>,
}

/// Records event <kind> for item <id> at <at>.
pub fn record(db: &Connection, id: Id, at: i64, kind: Kind, detail: Option<&str>) -> Result<()> {
    db.execute(
        "INSERT INTO item_event (item_id, at, kind, detail) VALUES (?1, ?2, ?3, ?4)",
        params![id, at, kind.word(), detail],
    )
    .sql()?;
    Ok(())
}

/// The events of each open item, in the order recorded.
fn read_open(db: &Connection) -> Result<BTreeMap<Id, Vec<Event>>> {
    let mut statement = db
        .prepare(
            "SELECT item_id, at, kind, detail FROM item_event \
             WHERE NOT EXISTS (SELECT 1 FROM closing WHERE record_id = item_id) \
             ORDER BY id",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .sql()?;
    let mut events: BTreeMap<Id, Vec<Event>> = BTreeMap::new();
    for row in rows {
        let (id, at, kind, detail) = row.sql()?;
        events.entry(id).or_default().push(Event {
            at,
            kind: Kind::from_word(&kind)?,
            detail,
        });
    }
    Ok(events)
}

/// The last recorded movement of each open item.
pub fn last_open(db: &Connection) -> Result<BTreeMap<Id, i64>> {
    let mut statement = db
        .prepare(
            "SELECT item_id, MAX(at) FROM item_event \
         WHERE NOT EXISTS (SELECT 1 FROM closing WHERE record_id = item_id) \
         GROUP BY item_id",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// The events of item <id>, open or closed, in the order recorded.
pub fn read_item(db: &Connection, id: Id) -> Result<Vec<Event>> {
    let mut statement = db
        .prepare("SELECT at, kind, detail FROM item_event WHERE item_id = ?1 ORDER BY id")
        .sql()?;
    let rows = statement
        .query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .sql()?;
    rows.map(|row| {
        let (at, kind, detail) = row.sql()?;
        Ok(Event {
            at,
            kind: Kind::from_word(&kind)?,
            detail,
        })
    })
    .collect()
}

/// When the store began to record events, or None when it has recorded them
/// since builds made it.
pub fn read_from(db: &Connection) -> Result<Option<i64>> {
    db.query_row("SELECT from_at FROM item_events_recorded", [], |row| {
        row.get(0)
    })
    .optional()
    .sql()
}

/// Marks the store as recording events from <at> on: the store move that made
/// the table calls this once.
pub fn start_recording(db: &Connection, at: i64) -> Result<()> {
    db.execute(
        "INSERT INTO item_events_recorded (singleton, from_at) VALUES (1, ?1)",
        [at],
    )
    .sql()?;
    Ok(())
}

/// The waits of an item that its events say are unmet: those gained and not
/// lost since.
fn noted_waits(events: &[Event]) -> BTreeSet<String> {
    let mut noted = BTreeSet::new();
    for event in events {
        let Some(wait) = &event.detail else { continue };
        match event.kind {
            Kind::WaitGained => {
                noted.insert(wait.clone());
            }
            Kind::WaitLost => {
                noted.remove(wait);
            }
            _ => {}
        }
    }
    noted
}

/// Records at <at> what the state <lists> holds changed for each open item
/// since its last events: each unmet wait it gained or lost, then its state
/// when that changed. An item opened before the store began to record events
/// and with none gets none. Every write runs this on the state it leaves
/// (store.rs).
pub fn follow(db: &Connection, lists: &Lists, at: i64) -> Result<()> {
    let from = read_from(db)?;
    let recorded = read_open(db)?;
    for item in &lists.items {
        let events = recorded.get(&item.id).map(Vec::as_slice).unwrap_or(&[]);
        if events.is_empty() && from.is_some_and(|from| item.opened.at < from) {
            continue;
        }
        let unmet: BTreeSet<String> = unmet_waits(lists, item)
            .into_iter()
            .map(Wait::arg)
            .collect();
        let noted = noted_waits(events);
        for lost in noted.difference(&unmet) {
            record(db, item.id, at, Kind::WaitLost, Some(lost))?;
        }
        for gained in unmet.difference(&noted) {
            record(db, item.id, at, Kind::WaitGained, Some(gained))?;
        }
        let (state, detail) = match item_state(lists, item) {
            ItemState::Running => (Kind::Running, None),
            ItemState::Backlog => (
                Kind::Backlog,
                item.backlog.as_ref().map(|backlog| backlog.why.as_str()),
            ),
            ItemState::Ready => (Kind::Ready, None),
            ItemState::Waiting if unmet.is_empty() && waits_for_others(lists, item) => {
                (Kind::Blocked, Some("placed last"))
            }
            ItemState::Waiting => (Kind::Blocked, None),
            ItemState::DesignSession => continue,
        };
        let last = events.iter().rev().find(|event| event.kind.is_state());
        if last.map(|event| (event.kind, event.detail.as_deref())) != Some((state, detail)) {
            record(db, item.id, at, state, detail)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "item_event_tests.rs"]
mod tests;
