//! The session that recorded a design change, and telling it the person
//! approved: `design record` and `design amend` keep the recording session's
//! inbox (inbox.rs) from their environment, and the dashboard's Approve button
//! sends "Design change #N of project P approved: land it." to that inbox,
//! once. The table is `design_session.sql`; this is the only code that writes
//! it. The inbox's token stays in the table and is never shown (inbox.rs).

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::inbox::{Messaging, Sent, Sessions, StoredEndpoint};
use crate::lists::design;
use crate::lists::store::{self, Store};
use crate::lists::{Id, Stamp};

/// What the dashboard and the lists JSON say of a design change's recording
/// session. Never the socket or the token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// No session was recorded with the change.
    Unrecorded,
    /// A session is recorded and has not been told.
    Waiting,
    /// Builds did not confirm that the notification reached the session.
    Unconfirmed,
    /// The approval reached the session's inbox.
    Told,
    /// Codex accepted the message but could not resume the thread.
    Queued,
    /// No session listens on the recorded socket any more.
    Ended,
}

impl SessionState {
    fn from_delivery(value: Option<&str>) -> Result<Self> {
        match value {
            None => Ok(Self::Waiting),
            Some("unconfirmed") => Ok(Self::Unconfirmed),
            Some("delivered") => Ok(Self::Told),
            Some("queued") => Ok(Self::Queued),
            Some("ended") => Ok(Self::Ended),
            Some(other) => fail!("Builds cannot read approval notification state {other:?}."),
        }
    }
}

/// Keeps <messaging> as the session of design change <id>, replacing the one
/// kept and clearing what it was told. With none (a record or amend outside a
/// session), an amend keeps the session it has, and clears what it was told.
pub fn keep(db: &Connection, id: Id, messaging: Option<&Messaging>) -> Result<()> {
    match messaging {
        Some(messaging) => {
            let (runtime, address, token) = messaging.stored();
            db.execute(
                "INSERT OR REPLACE INTO design_session (design_id, runtime, address, token, told) VALUES (?1, ?2, ?3, ?4, NULL)",
                params![id, runtime, address, token],
            )
            .sql()?;
        }
        None => {
            db.execute(
                "UPDATE design_session SET told = NULL WHERE design_id = ?1",
                [id],
            )
            .sql()?;
        }
    }
    Ok(())
}

/// Every design change's session state.
pub(in crate::lists) fn states(db: &Connection) -> Result<BTreeMap<Id, SessionState>> {
    let mut statement = db
        .prepare("SELECT design_id, told FROM design_session")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, Id>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .sql()?;
    let mut states = BTreeMap::new();
    for row in rows {
        let (id, told) = row.sql()?;
        states.insert(id, SessionState::from_delivery(told.as_deref())?);
    }
    Ok(states)
}

fn kept(db: &Connection, id: Id) -> Result<Option<Messaging>> {
    let row: Option<StoredEndpoint> = db
        .query_row(
            "SELECT runtime, address, token FROM design_session WHERE design_id = ?1",
            [id],
            StoredEndpoint::read,
        )
        .optional()
        .sql()?;
    row.map(StoredEndpoint::restore).transpose()
}

/// What approving did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    /// The change was approved before; nothing was written or sent.
    Already,
    /// The approval is recorded, and the session is as told.
    Recorded(SessionState),
}

/// The words the session is sent.
pub fn approved_words(project: &str, id: Id) -> String {
    format!(
        "Design change #{} of project {project} approved: land it.",
        id.0
    )
}

/// Records the person's approval of design change <id> in <store> as
/// `builds design approve N --by-person` does, then tells the session that
/// recorded it, once. The approval is written first and stands whatever the
/// send finds. The card reports unconfirmed delivery until builds records
/// a known outcome. The approval stands if notification fails.
pub fn approve_and_tell(
    store: &Store,
    sessions: &Sessions,
    id: Id,
    stamp: &Stamp,
) -> Result<Approval> {
    let to = store.write(stamp, &format!("design approve {id}"), |db| {
        if store::read(db)?
            .design(id)
            .is_some_and(|d| d.approved.is_some())
        {
            return Ok(None);
        }
        design::approve(db, id, stamp)?;
        let to = kept(db, id)?;
        if to.is_some() {
            db.execute(
                "UPDATE design_session SET told = 'unconfirmed' WHERE design_id = ?1",
                [id],
            )
            .sql()?;
        }
        Ok(Some(to))
    })?;
    let Some(to) = to else {
        return Ok(Approval::Already);
    };
    let Some(to) = to else {
        return Ok(Approval::Recorded(SessionState::Unrecorded));
    };
    let told = match sessions.send(&to, &approved_words(&store.project, id))? {
        Sent::Delivered => "delivered",
        Sent::Queued => "queued",
        Sent::Gone => "ended",
    };
    store.write(stamp, &format!("design told {id}"), |db| {
        db.execute(
            "UPDATE design_session SET told = ?2 WHERE design_id = ?1 AND told = 'unconfirmed'",
            params![id, told],
        )
        .sql()?;
        Ok(())
    })?;
    Ok(Approval::Recorded(SessionState::from_delivery(Some(told))?))
}

pub fn move_legacy_destination(db: &Connection) -> Result<()> {
    crate::inbox::move_legacy_destination(
        db,
        "design_session",
        crate::db::statement(
            crate::db::SCHEMA,
            "CREATE TABLE IF NOT EXISTS design_session (",
            ") STRICT;",
        ),
        "design_id",
        ", told",
    )
}

#[cfg(test)]
#[path = "design_session_tests.rs"]
mod tests;
