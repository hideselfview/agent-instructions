//! Where each question and item came from. A question or an item may name its
//! origin, the record it came from: a question, an item, a decision or a design
//! change (item.sql, question.sql). Its trail is the chain of origins from the
//! first record to it, and shows the tags of the records on it. Only the first
//! record of a trail carries a tag: tag.rs refuses a tag that a record's trail
//! already shows, and takes off the tags that a new origin's trail shows. A
//! record can't be its own origin, and a chain of origins can't loop.
//! item.rs and question.rs write the origins; this module reads them.

use std::fmt::Write;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::Lists;
use crate::lists::record::{self, Id, Record};

/// The SQL that names every pair of a record and an earlier record of its
/// trail as `earlier (record, earlier)`, for a statement to follow. Its UNION
/// keeps each pair once, so it ends even on a chain that loops.
pub const EARLIER: &str = "WITH RECURSIVE origin (record, earlier) AS ( \
         SELECT id, origin FROM item WHERE origin IS NOT NULL \
         UNION ALL SELECT id, origin FROM question WHERE origin IS NOT NULL), \
     earlier (record, earlier) AS ( \
         SELECT record, earlier FROM origin \
         UNION SELECT e.record, o.earlier FROM earlier e JOIN origin o ON o.record = e.earlier)";

/// Refuses <origin> as the origin of record <id>: it must be another record,
/// and one whose trail does not go through <id>.
pub fn require_origin(db: &Connection, id: Id, origin: Id) -> Result<()> {
    if record::standing(db, origin)?.is_none() {
        fail!("the origin {origin} is not a record");
    }
    if origin == id {
        fail!("{id} cannot come from itself");
    }
    let through: Option<i64> = db
        .query_row(
            &format!("{EARLIER} SELECT 1 FROM earlier WHERE record = ?1 AND earlier = ?2"),
            params![origin, id],
            |row| row.get(0),
        )
        .optional()
        .sql()?;
    if through.is_some() {
        fail!("{id} cannot come from {origin}: {origin} already comes from {id}");
    }
    Ok(())
}

/// The record whose trail already shows tag <tag> to record <id>: the earlier
/// record of its trail that carries it, or None.
pub fn earlier_with_tag(db: &Connection, id: Id, tag: &str) -> Result<Option<Id>> {
    db.query_row(
        &format!(
            "{EARLIER} SELECT t.record_id FROM earlier e JOIN record_tag t ON t.record_id = e.earlier \
             WHERE e.record = ?1 AND t.tag = ?2 LIMIT 1"
        ),
        params![id, tag],
        |row| row.get(0),
    )
    .optional()
    .sql()
}

/// A record's trail: the tags it shows, and the records before it from the
/// first to the one it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trail {
    /// The tags of the records on the trail, the first record's first.
    pub tags: Vec<String>,
    /// The earlier records, the first record first.
    pub earlier: Vec<Id>,
}

/// The origin and tags of record <id>, open or closed; a decision or a design
/// change has neither.
fn origin_and_tags(lists: &Lists, id: Id) -> (Option<Id>, &[String]) {
    if let Some(question) = lists.question(id) {
        return (question.origin, &question.tags);
    }
    if let Some(item) = lists.item(id) {
        return (item.origin, &item.tags);
    }
    match lists.closed_record(id).map(|closed| &closed.record) {
        Some(Record::Question(question)) => (question.origin, &question.tags),
        Some(Record::Item(item)) => (item.origin, &item.tags),
        _ => (None, &[]),
    }
}

/// The trail of record <id> in <lists>.
pub fn trail(lists: &Lists, id: Id) -> Trail {
    let mut chain = vec![id];
    let mut at = id;
    // The check refuses a loop; a store read inside a write, before the
    // check, still stops at the first record seen twice.
    while let (Some(origin), _) = origin_and_tags(lists, at) {
        if chain.contains(&origin) {
            break;
        }
        chain.push(origin);
        at = origin;
    }
    chain.reverse();
    let tags = chain
        .iter()
        .flat_map(|record| origin_and_tags(lists, *record).1.iter().cloned())
        .collect();
    chain.pop();
    Trail {
        tags,
        earlier: chain,
    }
}

impl Trail {
    /// Whether the trail shows tag <wanted>, or nothing is asked: what
    /// `--tag T` chooses records by.
    pub fn wanted(&self, wanted: Option<&str>) -> bool {
        wanted.is_none_or(|wanted| self.tags.iter().any(|tag| tag == wanted))
    }

    /// The trail of record <id> as one line, with the title of the record it
    /// came from only: `screen-reads audit → #3 → #50 Read each chained screen
    /// value at once → #51`. None when it shows no tag and no earlier record.
    pub fn line(&self, lists: &Lists, id: Id) -> Option<String> {
        if self.tags.is_empty() && self.earlier.is_empty() {
            return None;
        }
        let mut line = self.tags.join("; ");
        for (at, earlier) in self.earlier.iter().enumerate() {
            if !line.is_empty() {
                line.push_str(" → ");
            }
            let _ = write!(line, "{earlier}");
            if at + 1 == self.earlier.len()
                && let Some(found) = lists.locate(*earlier)
            {
                let _ = write!(line, " {}", found.title);
            }
        }
        if !line.is_empty() {
            line.push_str(" → ");
        }
        let _ = write!(line, "{id}");
        Some(line)
    }
}

/// The trail of record <id> as one line (`Trail::line`), after `trail: `.
pub fn trail_line(lists: &Lists, id: Id) -> Option<String> {
    trail(lists, id)
        .line(lists, id)
        .map(|line| format!("trail: {line}"))
}

#[cfg(test)]
#[path = "trail_tests.rs"]
mod tests;
