//! Tags: each names the audit or group a question or an item came from, such
//! as "screen-reads audit", so the person can see one group's records
//! together (tag.sql). A record has any number of tags, kept in the order of
//! their text, and keeps them when it closes, so a closed record can still be
//! tagged or untagged.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::record::{self, Id, Kind};

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

/// Gives record <id> each of <tags> it does not have yet.
pub fn add_tags(db: &Connection, id: Id, tags: &[String]) -> Result<()> {
    if tags.is_empty() {
        return Ok(());
    }
    let kind = taggable(db, id)?;
    for tag in tags {
        db.execute(
            "INSERT OR IGNORE INTO record_tag (record_id, kind, tag) VALUES (?1, ?2, ?3)",
            params![id, kind.word(), tag],
        )
        .sql()?;
    }
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

/// Whether a record with <tags> is one `--tag <wanted>` asks for: every
/// record when nothing is asked.
pub fn wanted(tags: &[String], wanted: Option<&str>) -> bool {
    wanted.is_none_or(|wanted| tags.iter().any(|tag| tag == wanted))
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
