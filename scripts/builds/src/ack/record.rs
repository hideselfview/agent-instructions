//! The acknowledgements and readings in a project's store (record.sql): what
//! `builds ack` writes and what the gate and the dashboard read.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;

/// How many versions of the text builds keeps: the ones agents acknowledged
/// most recently.
pub const KEPT_VERSIONS: i64 = 20;

/// An agent's last acknowledgement, as the gate reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ack {
    pub version: String,
    /// Seconds since 1970.
    pub at: i64,
}

/// An agent's last acknowledgement, as the dashboard lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAck {
    pub who: String,
    pub version: String,
    /// Seconds since 1970.
    pub at: i64,
}

/// The reading an agent started with `builds ack`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub version: String,
    /// The names of the sections the reading holds, in the order its parts
    /// show them.
    pub sections: Vec<String>,
    /// Each part's check words, in order, part 1 first.
    pub words: Vec<Vec<String>>,
}

/// The last acknowledgement of <who> in <session>.
pub fn last(db: &Connection, session: &str, who: &str) -> Result<Option<Ack>> {
    db.query_row(
        "SELECT version, at FROM ack WHERE session = ?1 AND who = ?2",
        params![session, who],
        |row| {
            Ok(Ack {
                version: row.get(0)?,
                at: row.get(1)?,
            })
        },
    )
    .optional()
    .sql()
}

/// Each section of version <version> of the text, by name, with its hash,
/// in the order of their names; None when builds no longer keeps that
/// version.
pub fn kept_sections(db: &Connection, version: &str) -> Result<Option<Vec<(String, String)>>> {
    let kept = db
        .query_row(
            "SELECT 1 FROM ack_version WHERE version = ?1",
            params![version],
            |_| Ok(()),
        )
        .optional()
        .sql()?;
    if kept.is_none() {
        return Ok(None);
    }
    let mut statement = db
        .prepare("SELECT name, hash FROM ack_version_section WHERE version = ?1 ORDER BY name")
        .sql()?;
    let sections = statement
        .query_map(params![version], |row| Ok((row.get(0)?, row.get(1)?)))
        .sql()?
        .collect::<rusqlite::Result<Vec<(String, String)>>>()
        .sql()?;
    Ok(Some(sections))
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
/// which holds the sections named <sections> in that order, with each part's
/// check words, in place of the reading it had.
pub fn start_reading(
    db: &Connection,
    session: &str,
    who: &str,
    version: &str,
    at: i64,
    sections: &[String],
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
    for (position, name) in sections.iter().enumerate() {
        db.execute(
            "INSERT INTO ack_reading_section (session, who, position, name) VALUES (?1, ?2, ?3, ?4)",
            params![session, who, position as i64 + 1, name],
        )
        .sql()?;
    }
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
            "SELECT name FROM ack_reading_section WHERE session = ?1 AND who = ?2 ORDER BY position",
        )
        .sql()?;
    let sections = statement
        .query_map(params![session, who], |row| row.get(0))
        .sql()?
        .collect::<rusqlite::Result<Vec<String>>>()
        .sql()?;
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
    Ok(Some(Reading {
        version,
        sections,
        words,
    }))
}

/// Records that <who> in <session> acknowledged version <version> at <at>,
/// whose sections had <sections> (name and hash), in place of its last
/// acknowledgement, and ends its reading. builds keeps the version with its
/// sections, and deletes each version that is no longer among the
/// `KEPT_VERSIONS` versions that agents acknowledged most recently.
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
    db.execute(
        "INSERT INTO ack_version (version, acknowledged_at) VALUES (?1, ?2)
         ON CONFLICT (version) DO UPDATE SET acknowledged_at = excluded.acknowledged_at",
        params![version, at],
    )
    .sql()?;
    for (name, hash) in sections {
        db.execute(
            "INSERT INTO ack_version_section (version, name, hash) VALUES (?1, ?2, ?3)
             ON CONFLICT (version, name) DO NOTHING",
            params![version, name, hash],
        )
        .sql()?;
    }
    db.execute(
        "DELETE FROM ack_version WHERE version NOT IN (
             SELECT version FROM ack_version ORDER BY acknowledged_at DESC, version LIMIT ?1
         )",
        params![KEPT_VERSIONS],
    )
    .sql()?;
    db.execute(
        "DELETE FROM ack_reading WHERE session = ?1 AND who = ?2",
        params![session, who],
    )
    .sql()?;
    Ok(())
}

/// Moves the acknowledgements' tables of a store from before builds kept the
/// versions of the text to today's, inside the move's transaction <tx>, with
/// foreign keys off. Each version that an acknowledgement names becomes a
/// version that builds keeps, with the sections that its acknowledgements
/// kept and the time an agent last acknowledged it, and the move keeps the
/// `KEPT_VERSIONS` versions that agents acknowledged most recently. The move deletes every reading in
/// progress, since an old reading held every section with the changed ones
/// first, an order the new tables do not record: each agent that was reading
/// starts again with `builds ack`.
pub fn move_to_kept_versions(tx: &Connection) -> Result<()> {
    let schema = crate::db::schema();
    for table in ["ack_version", "ack_version_section", "ack_reading_section"] {
        tx.execute_batch(crate::db::statement(
            &schema,
            &format!("CREATE TABLE {table} ("),
            "WITHOUT ROWID;",
        ))
        .sql()?;
    }
    tx.execute(
        "INSERT INTO ack_version (version, acknowledged_at)
             SELECT version, MAX(at) FROM ack GROUP BY version
             ORDER BY MAX(at) DESC, version LIMIT ?1",
        params![KEPT_VERSIONS],
    )
    .sql()?;
    tx.execute_batch(
        "INSERT INTO ack_version_section (version, name, hash)
             SELECT DISTINCT ack.version, ack_section.name, ack_section.hash
             FROM ack_section JOIN ack USING (session, who)
             WHERE ack.version IN (SELECT version FROM ack_version);
         DROP TABLE ack_section;
         DELETE FROM ack_word;
         DELETE FROM ack_reading;",
    )
    .sql()?;
    Ok(())
}

/// Every agent's last acknowledgement, newest first.
pub fn all(db: &Connection) -> Result<Vec<AgentAck>> {
    let mut statement = db
        .prepare("SELECT who, version, at FROM ack ORDER BY at DESC, who")
        .sql()?;
    statement
        .query_map([], |row| {
            Ok(AgentAck {
                who: row.get(0)?,
                version: row.get(1)?,
                at: row.get(2)?,
            })
        })
        .sql()?
        .collect::<rusqlite::Result<Vec<AgentAck>>>()
        .sql()
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
