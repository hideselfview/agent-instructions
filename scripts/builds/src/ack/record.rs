//! The acknowledgements and readings in a project's store (record.sql): what
//! `builds ack` writes and what the gate and the dashboard read.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;

/// An agent's last acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ack {
    pub session: String,
    pub who: String,
    pub version: String,
    /// Seconds since 1970.
    pub at: i64,
    /// Each section the text held, by name, with its hash.
    pub sections: Vec<(String, String)>,
}

/// The reading an agent started with `builds ack`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub version: String,
    /// Each part's check words, in order, part 1 first.
    pub words: Vec<Vec<String>>,
}

/// The last acknowledgement of <who> in <session>, with its sections.
pub fn last(db: &Connection, session: &str, who: &str) -> Result<Option<Ack>> {
    let Some((version, at)) = db
        .query_row(
            "SELECT version, at FROM ack WHERE session = ?1 AND who = ?2",
            params![session, who],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .sql()?
    else {
        return Ok(None);
    };
    let mut statement = db
        .prepare("SELECT name, hash FROM ack_section WHERE session = ?1 AND who = ?2 ORDER BY name")
        .sql()?;
    let sections = statement
        .query_map(params![session, who], |row| Ok((row.get(0)?, row.get(1)?)))
        .sql()?
        .collect::<rusqlite::Result<Vec<(String, String)>>>()
        .sql()?;
    Ok(Some(Ack {
        session: session.to_string(),
        who: who.to_string(),
        version,
        at,
        sections,
    }))
}

/// The name <session> last acknowledged or started a reading under: the
/// newest of its acknowledgements and readings. None when it has neither.
pub fn last_who(db: &Connection, session: &str) -> Result<Option<String>> {
    db.query_row(
        "SELECT who FROM (
             SELECT who, at FROM ack WHERE session = ?1
             UNION ALL
             SELECT who, started_at AS at FROM ack_reading WHERE session = ?1
         ) ORDER BY at DESC LIMIT 1",
        params![session],
        |row| row.get(0),
    )
    .optional()
    .sql()
}

/// Starts the reading of version <version> by <who> in <session> at <at>,
/// with each part's check words, in place of the reading it had.
pub fn start_reading(
    db: &Connection,
    session: &str,
    who: &str,
    version: &str,
    at: i64,
    words: &[Vec<String>],
) -> Result<()> {
    db.execute(
        "DELETE FROM ack_reading WHERE session = ?1 AND who = ?2",
        params![session, who],
    )
    .sql()?;
    db.execute(
        "INSERT INTO ack_reading (session, who, version, started_at) VALUES (?1, ?2, ?3, ?4)",
        params![session, who, version, at],
    )
    .sql()?;
    for (part, part_words) in words.iter().enumerate() {
        for (position, word) in part_words.iter().enumerate() {
            db.execute(
                "INSERT INTO ack_word (session, who, part, position, word) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session, who, part as i64 + 1, position as i64 + 1, word],
            )
            .sql()?;
        }
    }
    Ok(())
}

/// The reading <who> in <session> started, or None.
pub fn reading(db: &Connection, session: &str, who: &str) -> Result<Option<Reading>> {
    let Some(version) = db
        .query_row(
            "SELECT version FROM ack_reading WHERE session = ?1 AND who = ?2",
            params![session, who],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .sql()?
    else {
        return Ok(None);
    };
    let mut statement = db
        .prepare(
            "SELECT part, word FROM ack_word WHERE session = ?1 AND who = ?2 ORDER BY part, position",
        )
        .sql()?;
    let rows = statement
        .query_map(params![session, who], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?
        .collect::<rusqlite::Result<Vec<(i64, String)>>>()
        .sql()?;
    let mut words: Vec<Vec<String>> = Vec::new();
    for (part, word) in rows {
        while words.len() < part as usize {
            words.push(Vec::new());
        }
        words[part as usize - 1].push(word);
    }
    Ok(Some(Reading { version, words }))
}

/// Records that <who> in <session> acknowledged version <version> at <at>,
/// whose sections had <sections> (name and hash), in place of its last
/// acknowledgement, and ends its reading.
pub fn acknowledge(
    db: &Connection,
    session: &str,
    who: &str,
    version: &str,
    at: i64,
    sections: &[(String, String)],
) -> Result<()> {
    db.execute(
        "DELETE FROM ack WHERE session = ?1 AND who = ?2",
        params![session, who],
    )
    .sql()?;
    db.execute(
        "INSERT INTO ack (session, who, version, at) VALUES (?1, ?2, ?3, ?4)",
        params![session, who, version, at],
    )
    .sql()?;
    for (name, hash) in sections {
        db.execute(
            "INSERT INTO ack_section (session, who, name, hash) VALUES (?1, ?2, ?3, ?4)",
            params![session, who, name, hash],
        )
        .sql()?;
    }
    db.execute(
        "DELETE FROM ack_reading WHERE session = ?1 AND who = ?2",
        params![session, who],
    )
    .sql()?;
    Ok(())
}

/// Deletes every acknowledgement and reading of <session>, after Claude Code
/// compacted it. Returns how many acknowledgements it deleted.
pub fn forget_session(db: &Connection, session: &str) -> Result<usize> {
    let deleted = db
        .execute("DELETE FROM ack WHERE session = ?1", params![session])
        .sql()?;
    db.execute(
        "DELETE FROM ack_reading WHERE session = ?1",
        params![session],
    )
    .sql()?;
    Ok(deleted)
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
