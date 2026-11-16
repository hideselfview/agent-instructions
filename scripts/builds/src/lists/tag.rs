//! Tags: each names the audit or group a question or an item came from, such
//! as "screen-reads audit", so the person can see one group's records
//! together (tag.sql). A record has any number of tags, kept in the order of
//! their text, and keeps them when it closes, so a closed record can still be
//! tagged or untagged. Only the first record of a trail that has a tag
//! carries it (trail.rs): every later record's trail has the tag, so a record
//! never carries a tag that an earlier record of its trail carries.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::record::{self, Id, Kind};
use crate::lists::trail;

/// Why <tag> is not a tag: a tag is one line with no spaces at its ends.
pub fn tag_fault(tag: &str) -> Option<String> {
    if tag.trim().is_empty() {
        Some("a tag is empty".to_string())
    } else if tag.contains(['\n', '\r', '\t']) {
        Some(format!(
            "tag '{}' is not one line",
            tag.replace(['\n', '\r', '\t'], " ")
        ))
    } else if tag != tag.trim() {
        Some(format!("tag '{tag}' has spaces at its ends"))
    } else {
        None
    }
}

/// <tags> as given on the command line, each checked, refused when one is not
/// a tag.
pub fn parse_tags(tags: Vec<&str>) -> Result<Vec<String>> {
    tags.into_iter()
        .map(|tag| match tag_fault(tag) {
            Some(fault) => fail!("{fault}"),
            None => Ok(tag.to_string()),
        })
        .collect()
}

/// The kind of record <id>, which must be a question or an item, open or
/// closed.
fn taggable(db: &Connection, id: Id) -> Result<Kind> {
    match record::standing(db, id)? {
        Some((kind @ (Kind::Question | Kind::Item), _)) => Ok(kind),
        Some((kind, _)) => fail!(
            "{id} is a {}; only questions and items take tags",
            kind.name()
        ),
        None => fail!("{id} is not a record"),
    }
}

/// Gives record <id> each of <tags> it does not have yet. Refuses a tag that
/// an earlier record of its trail carries, naming that record, and removes
/// each new tag from the later records that carried it.
pub fn add_tags(db: &Connection, id: Id, tags: &[String]) -> Result<()> {
    if tags.is_empty() {
        return Ok(());
    }
    let kind = taggable(db, id)?;
    for tag in tags {
        if let Some(on) = trail::earlier_with_tag(db, id, tag)? {
            fail!("{on}, earlier on the trail of {id}, already carries the tag '{tag}'");
        }
        db.execute(
            "INSERT OR IGNORE INTO record_tag (record_id, kind, tag) VALUES (?1, ?2, ?3)",
            params![id, kind.word(), tag],
        )
        .sql()?;
    }
    drop_repeated(db)
}

/// Removes from each record every tag that an earlier record of its trail
/// carries. A new origin, or a tag on an earlier record, can make a record
/// repeat such a tag.
pub fn drop_repeated(db: &Connection) -> Result<()> {
    db.execute(
        &format!(
            "{} DELETE FROM record_tag WHERE EXISTS (SELECT 1 FROM earlier e \
             JOIN record_tag t ON t.record_id = e.earlier AND t.tag = record_tag.tag \
             WHERE e.record = record_tag.record_id)",
            trail::EARLIER
        ),
        [],
    )
    .sql()?;
    Ok(())
}

/// Takes each of <tags> off record <id>; refuses a tag it does not have.
pub fn remove_tags(db: &Connection, id: Id, tags: &[String]) -> Result<()> {
    if tags.is_empty() {
        return Ok(());
    }
    taggable(db, id)?;
    for tag in tags {
        let removed = db
            .execute(
                "DELETE FROM record_tag WHERE record_id = ?1 AND tag = ?2",
                params![id, tag],
            )
            .sql()?;
        if removed == 0 {
            fail!("{id} has no tag '{tag}'");
        }
    }
    Ok(())
}

/// Every record's tags, by id, each record's in the order of their text.
pub fn read(db: &Connection) -> Result<BTreeMap<Id, Vec<String>>> {
    let mut statement = db
        .prepare("SELECT record_id, tag FROM record_tag ORDER BY record_id, tag")
        .sql()?;
    let mut tags: BTreeMap<Id, Vec<String>> = BTreeMap::new();
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?
    {
        let (id, tag) = row.sql()?;
        tags.entry(id).or_default().push(tag);
    }
    Ok(tags)
}

/// The tables tag.rs owns.
#[cfg(test)]
const TABLES: [&str; 1] = ["record_tag"];

/// Writes a row of tag.rs's table as given, the constraints still holding:
/// for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "tag_tests.rs"]
mod tests;
