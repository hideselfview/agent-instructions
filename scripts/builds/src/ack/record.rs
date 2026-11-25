//! The acknowledgements and readings in a project's store (record.sql): what
//! `builds ack` writes and what the gate reads.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Sql;
use crate::fail::Result;

/// The reading an agent started with `builds ack`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub version: String,
    /// The tokens, the opening output's first, then each part's (sql file).
    /// One more than the reading has parts.
    pub tokens: Vec<String>,
}

/// The version of the text that <agent> in <session> last acknowledged.
pub fn last(db: &Connection, session: &str, agent: &str) -> Result<Option<String>> {
    db.query_row(
        "SELECT version FROM ack WHERE session = ?1 AND agent = ?2",
        params![session, agent],
        |row| row.get(0),
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

/// The sections, each by name and hash, of the version that <agent> in
/// <session> last acknowledged in this store, or none when the agent has no
/// acknowledgement here or builds no longer keeps that version (a store from
/// before `acknowledge` kept every acknowledged version).
pub fn acknowledged_sections(
    db: &Connection,
    session: &str,
    agent: &str,
) -> Result<Vec<(String, String)>> {
    match last(db, session, agent)? {
        Some(version) => Ok(kept_sections(db, &version)?.unwrap_or_default()),
        None => Ok(Vec::new()),
    }
}

/// Starts the reading of version <version> by <agent> in <session> at <at>,
/// with <tokens>, in place of the reading it had.
pub fn start_reading(
    db: &Connection,
    session: &str,
    agent: &str,
    version: &str,
    at: i64,
    tokens: &[String],
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
    for (part, token) in tokens.iter().enumerate() {
        db.execute(
            "INSERT INTO ack_token (session, agent, part, token) VALUES (?1, ?2, ?3, ?4)",
            params![session, agent, part as i64, token],
        )
        .sql()?;
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
        .prepare("SELECT token FROM ack_token WHERE session = ?1 AND agent = ?2 ORDER BY part")
        .sql()?;
    let tokens = statement
        .query_map(params![session, agent], |row| row.get(0))
        .sql()?
        .collect::<rusqlite::Result<Vec<String>>>()
        .sql()?;
    Ok(Some(Reading { version, tokens }))
}

/// Records that <agent> in <session> acknowledged version <version> at <at>
/// of the text, whose sections had <sections> (name and hash), in place of
/// its last acknowledgement, and ends its reading. builds keeps the version
/// with its sections and the time, and deletes each version that no agent's
/// acknowledgement names: an agent's roles are the ones whose sections its
/// version holds, so builds keeps that version.
pub fn acknowledge(
    db: &Connection,
    session: &str,
    agent: &str,
    version: &str,
    at: i64,
    sections: &[(String, String)],
) -> Result<()> {
    db.execute(
        "DELETE FROM ack WHERE session = ?1 AND agent = ?2",
        params![session, agent],
    )
    .sql()?;
    db.execute(
        "INSERT INTO ack (session, agent, version) VALUES (?1, ?2, ?3)",
        params![session, agent, version],
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
        "DELETE FROM ack_version WHERE version NOT IN (SELECT version FROM ack)",
        [],
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

/// Makes `ack` again as record.sql writes it, with no acknowledgements: a
/// store from before keyed each acknowledgement by the repo whose CLAUDE.md
/// the text held, and the text no longer holds one, so each agent reads the
/// text again.
pub fn drop_repo(db: &Connection) -> Result<()> {
    crate::db::remake_table(
        db,
        "ack",
        crate::db::statement(
            include_str!("record.sql"),
            "CREATE TABLE ack (",
            ") STRICT, WITHOUT ROWID;",
        ),
        |_| Ok(()),
        &[],
    )
}

/// Makes `ack` again as record.sql writes it, with every acknowledgement
/// kept, and drops the table of the sections a reading held: nothing reads
/// the time of an acknowledgement or those sections any more. migrate.rs runs
/// a move with the foreign keys off.
pub fn drop_unread(db: &Connection) -> Result<()> {
    db.execute_batch("DROP TABLE ack_reading_section;").sql()?;
    crate::db::remake_table(
        db,
        "ack",
        crate::db::statement(
            include_str!("record.sql"),
            "CREATE TABLE ack (",
            ") STRICT, WITHOUT ROWID;",
        ),
        |db| {
            db.execute_batch(
                "INSERT INTO ack (session, agent, version) SELECT session, agent, version FROM ack_old;",
            )
            .sql()
        },
        &[],
    )
}

/// Deletes every reading, which held made-up words, and replaces the table of
/// the words with the table of the tokens, for a store move. migrate.rs runs
/// a move with the foreign keys off, so this deletes the readings' rows of
/// each table that points at them.
pub fn replace_words_with_tokens(db: &Connection) -> Result<()> {
    db.execute_batch(
        "DROP TABLE ack_word;
         DELETE FROM ack_reading_section;
         DELETE FROM ack_reading;",
    )
    .sql()?;
    db.execute_batch(crate::db::statement(
        include_str!("record.sql"),
        "CREATE TABLE ack_token (",
        ") STRICT, WITHOUT ROWID;",
    ))
    .sql()
}

#[cfg(test)]
#[path = "record_tests.rs"]
pub mod tests;
