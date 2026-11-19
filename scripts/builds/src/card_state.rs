//! The changes of each worktree card's dashboard state (card_state.sql).
//! `follow` writes a project's changes from the cards the daemon judged
//! (daemon/card_recorder.rs), and `read_item` reads back the stretches of
//! state an item's cards spent, for `builds item times`.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::lists::Id;

/// How many days a store keeps a row that is not its worktree's last.
pub const KEEP_DAYS: i64 = 30;

/// The state of a card that no longer shows under the project.
pub const GONE: &str = "gone";

/// A card as the daemon judged it: its worktree, its state's name and the
/// ids of the items it lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub worktree: String,
    pub state: &'static str,
    pub items: BTreeSet<u32>,
}

/// The last row of each worktree: its state and items.
fn last_rows(db: &Connection) -> Result<BTreeMap<String, (String, BTreeSet<u32>)>> {
    let mut statement = db
        .prepare(
            "SELECT s.worktree, s.state, i.item_id FROM card_state s \
             LEFT JOIN card_state_item i ON i.state_id = s.id \
             WHERE s.id IN (SELECT MAX(id) FROM card_state GROUP BY worktree)",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<u32>>(2)?,
            ))
        })
        .sql()?;
    let mut last: BTreeMap<String, (String, BTreeSet<u32>)> = BTreeMap::new();
    for row in rows {
        let (worktree, state, item) = row.sql()?;
        let entry = last.entry(worktree).or_insert((state, BTreeSet::new()));
        entry.1.extend(item);
    }
    Ok(last)
}

fn insert(
    db: &Connection,
    at: i64,
    worktree: &str,
    state: &str,
    items: &BTreeSet<u32>,
) -> Result<()> {
    db.execute(
        "INSERT INTO card_state (worktree, at, state) VALUES (?1, ?2, ?3)",
        params![worktree, at, state],
    )
    .sql()?;
    let id = db.last_insert_rowid();
    for item in items {
        db.execute(
            "INSERT INTO card_state_item (state_id, item_id) VALUES (?1, ?2)",
            params![id, item],
        )
        .sql()?;
    }
    Ok(())
}

/// Records at <at>, in one transaction, each card of <cards>, the cards that
/// show under this store's project now, whose state or items differ from its
/// worktree's last row, and `GONE` for each worktree whose last row is a
/// card's state that is not in <cards> nor in <unread>, the worktrees whose
/// project the daemon could not choose. Then deletes the rows older than
/// `KEEP_DAYS` days that are not their worktree's last.
pub fn follow(
    db: &mut Connection,
    at: i64,
    cards: &[Seen],
    unread: &BTreeSet<String>,
) -> Result<()> {
    let tx = db.transaction().sql()?;
    let last = last_rows(&tx)?;
    for card in cards {
        let same = last
            .get(&card.worktree)
            .is_some_and(|(state, items)| state == card.state && *items == card.items);
        if !same {
            insert(&tx, at, &card.worktree, card.state, &card.items)?;
        }
    }
    for (worktree, (state, _)) in &last {
        let shown = cards.iter().any(|card| card.worktree == *worktree);
        if state != GONE && !shown && !unread.contains(worktree) {
            insert(&tx, at, worktree, GONE, &BTreeSet::new())?;
        }
    }
    tx.execute(
        "DELETE FROM card_state WHERE at < ?1 \
         AND id NOT IN (SELECT MAX(id) FROM card_state GROUP BY worktree)",
        [at - KEEP_DAYS * 86400],
    )
    .sql()?;
    tx.commit().sql()
}

/// A stretch of time a card that listed an item spent in one state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub worktree: String,
    pub state: String,
    pub start: i64,
    /// When the worktree's next row began, or <until> for its last row.
    pub end: i64,
}

/// The stretches of the cards that listed item <id>, in time order, a
/// worktree's last row ending at <until>.
pub fn read_item(db: &Connection, id: Id, until: i64) -> Result<Vec<Stretch>> {
    let mut statement = db
        .prepare(
            "SELECT s.worktree, s.state, s.at, \
                    (SELECT n.at FROM card_state n WHERE n.worktree = s.worktree AND n.id > s.id \
                     ORDER BY n.id LIMIT 1) \
             FROM card_state s JOIN card_state_item i ON i.state_id = s.id \
             WHERE i.item_id = ?1 ORDER BY s.at, s.id",
        )
        .sql()?;
    let rows = statement
        .query_map([id], |row| {
            Ok(Stretch {
                worktree: row.get(0)?,
                state: row.get(1)?,
                start: row.get(2)?,
                end: row.get::<_, Option<i64>>(3)?.unwrap_or(until),
            })
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

#[cfg(test)]
#[path = "card_state_tests.rs"]
mod tests;
