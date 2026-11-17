//! The acknowledgements and readings in a project's store (record.sql): what
//! `builds ack` writes and what the gate reads.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;

/// How many versions of the text builds keeps: the ones agents acknowledged
/// most recently.
pub const KEPT_VERSIONS: i64 = 20;

/// An agent's last acknowledgement of one text, as the gate reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ack {
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

/// The last acknowledgement by <agent> in <session> of the text that holds
/// the CLAUDE.md of <repo>, or of the text that holds no repo's CLAUDE.md
/// when <repo> is None.
pub fn last(
    db: &Connection,
    session: &str,
    agent: &str,
    repo: Option<&str>,
) -> Result<Option<Ack>> {
    db.query_row(
        "SELECT version, at FROM ack WHERE session = ?1 AND agent = ?2 AND repo IS ?3",
        params![session, agent, repo],
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

/// Starts the reading of version <version> by <agent> in <session> at <at>,
/// which holds the sections named <sections> in that order, with each part's
/// check words, in place of the reading it had.
pub fn start_reading(
    db: &Connection,
    session: &str,
    agent: &str,
    version: &str,
    at: i64,
    sections: &[String],
    words: &[Vec<String>],
) -> Result<()> {
    db.execute(
        "DELETE FROM ack_reading WHERE session = ?1 AND agent = ?2",
        params![session, agent],
    )
    .sql()?;
    db.execute(
        "INSERT INTO ack_reading (session, agent, version, started_at) VALUES (?1, ?2, ?3, ?4)",
        params![session, agent, version, at],
    )
    .sql()?;
    for (position, name) in sections.iter().enumerate() {
        db.execute(
            "INSERT INTO ack_reading_section (session, agent, position, name) VALUES (?1, ?2, ?3, ?4)",
            params![session, agent, position as i64 + 1, name],
        )
        .sql()?;
    }
    for (part, part_words) in words.iter().enumerate() {
        for (position, word) in part_words.iter().enumerate() {
            db.execute(
                "INSERT INTO ack_word (session, agent, part, position, word) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session, agent, part as i64 + 1, position as i64 + 1, word],
            )
            .sql()?;
        }
    }
    Ok(())
}

/// The reading <agent> in <session> started, or None.
pub fn reading(db: &Connection, session: &str, agent: &str) -> Result<Option<Reading>> {
    let Some(version) = db
        .query_row(
            "SELECT version FROM ack_reading WHERE session = ?1 AND agent = ?2",
            params![session, agent],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .sql()?
    else {
        return Ok(None);
    };
    let mut statement = db
        .prepare(
            "SELECT name FROM ack_reading_section WHERE session = ?1 AND agent = ?2 ORDER BY position",
        )
        .sql()?;
    let sections = statement
        .query_map(params![session, agent], |row| row.get(0))
        .sql()?
        .collect::<rusqlite::Result<Vec<String>>>()
        .sql()?;
    let mut statement = db
        .prepare(
            "SELECT part, word FROM ack_word WHERE session = ?1 AND agent = ?2 ORDER BY part, position",
        )
        .sql()?;
    let rows = statement
        .query_map(params![session, agent], |row| {
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

/// Records that <agent> in <session> acknowledged version <version> at <at>
/// of the text that holds the CLAUDE.md of <repo> (None: of no repo), whose
/// sections had <sections> (name and hash), in place of its last
/// acknowledgement of that text, and ends its reading. The agent's
/// acknowledgements of other texts stay. builds keeps the version with its
/// sections, and deletes each version that is no longer among the
/// `KEPT_VERSIONS` versions that agents acknowledged most recently.
pub fn acknowledge(
    db: &Connection,
    session: &str,
    agent: &str,
    repo: Option<&str>,
    version: &str,
    at: i64,
    sections: &[(String, String)],
) -> Result<()> {
    db.execute(
        "DELETE FROM ack WHERE session = ?1 AND agent = ?2 AND repo IS ?3",
        params![session, agent, repo],
    )
    .sql()?;
    db.execute(
        "INSERT INTO ack (session, agent, repo, version, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session, agent, repo, version, at],
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
        "DELETE FROM ack_reading WHERE session = ?1 AND agent = ?2",
        params![session, agent],
    )
    .sql()?;
    Ok(())
}

/// Deletes every acknowledgement and the reading of agent <agent> of
/// <session>, after Claude Code compacted that agent.
pub fn forget(db: &Connection, session: &str, agent: &str) -> Result<()> {
    db.execute(
        "DELETE FROM ack WHERE session = ?1 AND agent = ?2",
        params![session, agent],
    )
    .sql()?;
    db.execute(
        "DELETE FROM ack_reading WHERE session = ?1 AND agent = ?2",
        params![session, agent],
    )
    .sql()?;
    Ok(())
}

/// Moves the acknowledgements' tables of a store from before builds named
/// each agent by its Claude Code agent id to today's, inside the move's
/// transaction <tx>: the move drops the old tables, whose rows name no agent
/// id, and makes today's empty, so every agent reads the whole text once.
pub fn move_to_agent_ids(tx: &Connection) -> Result<()> {
    tx.execute_batch(
        "DROP TABLE ack_word;
         DROP TABLE ack_reading;
         DROP TABLE ack_section;
         DROP TABLE ack;",
    )
    .sql()?;
    tx.execute_batch(crate::db::ACK_PART).sql()?;
    Ok(())
}

/// Moves the acknowledgements' table of a store from before builds kept an
/// agent's acknowledgement of each repo's text apart to today's, inside the
/// move's transaction <tx>: the move drops the old table, whose rows name no
/// repo, and makes today's empty, so every agent reads the text of each repo
/// it works in once. The versions builds keeps and the readings stay.
pub fn move_to_ack_per_repo(tx: &Connection) -> Result<()> {
    let schema = crate::db::schema();
    tx.execute_batch("DROP TABLE ack;").sql()?;
    tx.execute_batch(crate::db::statement(
        &schema,
        "CREATE TABLE ack (",
        "STRICT;",
    ))
    .sql()?;
    tx.execute_batch(crate::db::statement(
        &schema,
        "CREATE UNIQUE INDEX ack_of_repo ",
        ";",
    ))
    .sql()?;
    tx.execute_batch(crate::db::statement(
        &schema,
        "CREATE UNIQUE INDEX ack_of_no_repo ",
        ";",
    ))
    .sql()?;
    Ok(())
}

/// Writes a row of record.rs's table <table> as given, the constraints still
/// holding: for a test of a store move that needs a row of an older shape.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(
        db,
        &[
            "ack",
            "ack_version",
            "ack_version_section",
            "ack_reading",
            "ack_reading_section",
            "ack_word",
        ],
        table,
        columns,
    )
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
