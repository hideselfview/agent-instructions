//! The session that recorded a design change, and telling it the person
//! approved: `design record` and `design amend` keep the recording session's
//! inbox socket and token from their environment, and the dashboard's Approve
//! button sends "Design change #N of project P approved: land it." to that socket, once.
//! The table is `design_session.sql`; this is the only code that writes it.
//!
//! The token is a per-session secret. It lives in the table and in the
//! `Messaging` that carries it to the one send. No type here serializes it,
//! `Messaging`'s `Debug` leaves it out, and no message, log line or page names
//! it.
//!
//! The socket protocol is Claude Code's cross-session messaging
//! (https://code.claude.com/docs/en/cross-session-messaging.md, "The session's
//! inbox socket"): a connection to the socket, an optional first line
//! `{"type":"auth","token":"<token>"}`, then one newline-ended JSON line per
//! message. The docs give the auth line; the message line,
//! `{"type":"user","message":{"role":"user","content":"<text>"}}`, comes from
//! Claude Code's own help text and was tried against a live session.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::design;
use crate::lists::store::{self, Store};
use crate::lists::{Id, Stamp};

/// How long a write to the socket may wait. A session that is busy still
/// reads its inbox between its tool calls, so a send that stalls is a fault.
const SEND_WAIT: Duration = Duration::from_secs(10);

/// A session's inbox: its socket and, when it has one, its token.
#[derive(Clone, PartialEq, Eq)]
pub struct Messaging {
    socket: String,
    token: Option<String>,
}

impl std::fmt::Debug for Messaging {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Messaging")
            .field("socket", &self.socket)
            .field("token", &self.token.as_ref().map(|_| "(kept)"))
            .finish()
    }
}

impl Messaging {
    pub fn new(socket: &str, token: Option<&str>) -> Result<Messaging> {
        if !socket.starts_with('/') {
            fail!("CLAUDE_CODE_MESSAGING_SOCKET is not an absolute path");
        }
        Ok(Messaging {
            socket: socket.to_string(),
            token: token.filter(|t| !t.trim().is_empty()).map(str::to_string),
        })
    }

    /// The inbox of the session running this command: None outside a
    /// session, or in one without messaging.
    pub fn from_env() -> Result<Option<Messaging>> {
        let Some(socket) = std::env::var("CLAUDE_CODE_MESSAGING_SOCKET")
            .ok()
            .filter(|s| !s.trim().is_empty())
        else {
            return Ok(None);
        };
        let token = std::env::var("CLAUDE_CODE_MESSAGING_TOKEN").ok();
        Messaging::new(&socket, token.as_deref()).map(Some)
    }
}

/// What the dashboard and the lists JSON say of a design change's recording
/// session. Never the socket or the token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// No session was recorded with the change.
    Unrecorded,
    /// A session is recorded and has not been told.
    Waiting,
    /// The approval reached the session's inbox.
    Told,
    /// No session listens on the recorded socket any more.
    Ended,
}

/// Keeps <messaging> as the session of design change <id>, replacing the one
/// kept and clearing what it was told. With none (a record or amend outside a
/// session), an amend keeps the session it has, and clears what it was told.
pub fn keep(db: &Connection, id: Id, messaging: Option<&Messaging>) -> Result<()> {
    match messaging {
        Some(messaging) => {
            db.execute(
                "INSERT OR REPLACE INTO design_session (design_id, socket, token, told) VALUES (?1, ?2, ?3, NULL)",
                params![id, messaging.socket, messaging.token],
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
        let state = match told.as_deref() {
            None => SessionState::Waiting,
            Some("delivered") => SessionState::Told,
            Some(_) => SessionState::Ended,
        };
        states.insert(id, state);
    }
    Ok(states)
}

fn kept(db: &Connection, id: Id) -> Result<Option<Messaging>> {
    let mut statement = db
        .prepare("SELECT socket, token FROM design_session WHERE design_id = ?1")
        .sql()?;
    let mut rows = statement
        .query_map([id], |row| {
            Ok(Messaging {
                socket: row.get(0)?,
                token: row.get(1)?,
            })
        })
        .sql()?;
    rows.next().transpose().sql()
}

/// Whether a message reached the inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Delivered,
    /// Nothing listens on the socket: the session ended.
    Gone,
}

/// Sends <text> to <to>: the auth line when it has a token, then the message
/// line. A socket with no file or no listener is `Gone`; any other failure is
/// the error. The message is sent once and never again.
pub fn send(to: &Messaging, text: &str) -> Result<Sent> {
    let mut stream = match UnixStream::connect(&to.socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(Sent::Gone);
        }
        Err(error) => fail!("connecting to the session's inbox: {error}"),
    };
    let wrote = (|| -> std::io::Result<()> {
        stream.set_write_timeout(Some(SEND_WAIT))?;
        if let Some(token) = &to.token {
            writeln!(
                stream,
                "{}",
                serde_json::json!({ "type": "auth", "token": token })
            )?;
        }
        writeln!(
            stream,
            "{}",
            serde_json::json!({ "type": "user", "message": { "role": "user", "content": text } })
        )?;
        stream.flush()?;
        stream.shutdown(std::net::Shutdown::Write)
    })();
    match wrote {
        Ok(()) => Ok(Sent::Delivered),
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(Sent::Gone),
        Err(error) => fail!("sending to the session's inbox: {error}"),
    }
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
/// send finds; a send that fails other than by the session being gone is the
/// error, with the change approved and the session not told, and nothing
/// retries it.
pub fn approve_and_tell(store: &Store, id: Id, stamp: &Stamp) -> Result<Approval> {
    let to = store.write(stamp, &format!("design approve {id}"), |db| {
        if store::read(db)?
            .design(id)
            .is_some_and(|d| d.approved.is_some())
        {
            return Ok(None);
        }
        design::approve(db, id, stamp)?;
        Ok(Some(kept(db, id)?))
    })?;
    let Some(to) = to else {
        return Ok(Approval::Already);
    };
    let Some(to) = to else {
        return Ok(Approval::Recorded(SessionState::Unrecorded));
    };
    let told = match send(&to, &approved_words(&store.project, id))? {
        Sent::Delivered => "delivered",
        Sent::Gone => "ended",
    };
    store.write(stamp, &format!("design told {id}"), |db| {
        db.execute(
            "UPDATE design_session SET told = ?2 WHERE design_id = ?1",
            params![id, told],
        )
        .sql()?;
        Ok(())
    })?;
    Ok(Approval::Recorded(match told {
        "delivered" => SessionState::Told,
        _ => SessionState::Ended,
    }))
}

#[cfg(test)]
#[path = "design_session_tests.rs"]
mod tests;
