//! What someone changed in an open item that its agent has to read before the
//! work lands (item_change.sql): an edit of the item's details, and a decision
//! that became its origin. `builds item edit` records a change in the same
//! write as the edit. A landing that finishes the item refuses while a change
//! made since the item's take is unread (landing.rs), and `builds item
//! changes N` prints those changes whole and records that the agent read
//! them.

use std::collections::BTreeMap;
use std::fmt::Write;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::{Id, Item, Lists, Stamp};
use crate::text::local_minute;

/// What a change did to the item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum What {
    /// The item's details went from `old` to `new`.
    Details { old: String, new: String },
    /// The decision became the item's origin.
    Decision(Id),
}

/// One change of an item, and whether its agent read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub id: i64,
    pub made: Stamp,
    pub what: What,
    /// Who read it and when, once an agent ran `builds item changes`.
    pub read: Option<Stamp>,
}

/// Records that <stamp> changed the details of item <id> from <old> to <new>;
/// records nothing when they are the same.
pub fn record_details(db: &Connection, id: Id, old: &str, new: &str, stamp: &Stamp) -> Result<()> {
    if old == new {
        return Ok(());
    }
    db.execute(
        "INSERT INTO item_change (item_id, at, by, kind, old_details, new_details) \
         VALUES (?1, ?2, ?3, 'details', ?4, ?5)",
        params![id, stamp.at, stamp.by, old, new],
    )
    .sql()?;
    Ok(())
}

/// Records that <stamp> made decision <decision> the origin of item <id>.
pub fn record_decision(db: &Connection, id: Id, decision: Id, stamp: &Stamp) -> Result<()> {
    db.execute(
        "INSERT INTO item_change (item_id, at, by, kind, decision_id) \
         VALUES (?1, ?2, ?3, 'decision', ?4)",
        params![id, stamp.at, stamp.by, decision],
    )
    .sql()?;
    Ok(())
}

/// Records that <stamp> read each change of <ids>.
pub fn mark_read(db: &Connection, ids: &[i64], stamp: &Stamp) -> Result<()> {
    for id in ids {
        db.execute(
            "INSERT INTO item_change_read (change_id, at, by) VALUES (?1, ?2, ?3)",
            params![id, stamp.at, stamp.by],
        )
        .sql()?;
    }
    Ok(())
}

/// Every item's changes, oldest first.
pub fn read_all(db: &Connection) -> Result<BTreeMap<Id, Vec<Change>>> {
    let mut statement = db
        .prepare(
            "SELECT c.id, c.item_id, c.at, c.by, c.kind, c.old_details, c.new_details, c.decision_id, \
                    r.at, r.by \
             FROM item_change c LEFT JOIN item_change_read r ON r.change_id = c.id ORDER BY c.id",
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
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<Id>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .sql()?;
    let mut all: BTreeMap<Id, Vec<Change>> = BTreeMap::new();
    for row in rows {
        let (id, item, made, kind, old, new, decision, read_at, read_by) = row.sql()?;
        let what = match (kind.as_str(), old, new, decision) {
            ("details", Some(old), Some(new), None) => What::Details { old, new },
            ("decision", None, None, Some(decision)) => What::Decision(decision),
            _ => fail!("the store holds a malformed change {id} of item {item}"),
        };
        let read = match (read_at, read_by) {
            (Some(at), Some(by)) => Some(Stamp { at, by }),
            _ => None,
        };
        all.entry(item).or_default().push(Change {
            id,
            made,
            what,
            read,
        });
    }
    Ok(all)
}

impl Item {
    /// The changes made since the run that goes on began that no agent read:
    /// none when the item does not run.
    pub fn unread_changes(&self) -> Vec<&Change> {
        let Some(since) = self.running.iter().map(|place| place.since.at).min() else {
            return Vec::new();
        };
        self.changes
            .iter()
            .filter(|change| change.made.at >= since && change.read.is_none())
            .collect()
    }
}

/// <change> whole, as an agent reads it: who made it and when, then the old
/// and new details, or the decision's title and body.
pub fn words(lists: &Lists, change: &Change) -> String {
    let mut out = String::new();
    let when = format!("{} by {}", local_minute(change.made.at), change.made.by);
    match &change.what {
        What::Details { old, new } => {
            let _ = write!(
                out,
                "details edited {when}\n--- details before ---\n{}\n--- details after ---\n{}\n",
                old.trim_end(),
                new.trim_end()
            );
        }
        What::Decision(id) => match lists.decision(*id) {
            Some(decision) => {
                let _ = write!(
                    out,
                    "decision {id} made the item's origin {when}\n{}\n{}\n",
                    decision.title,
                    decision.body.trim_end()
                );
            }
            None => {
                let _ = writeln!(out, "decision {id} made the item's origin {when}");
            }
        },
    }
    out
}

#[cfg(test)]
#[path = "item_change_tests.rs"]
mod tests;
