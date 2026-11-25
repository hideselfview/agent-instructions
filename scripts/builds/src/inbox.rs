//! Notifications to a saved Codex thread or to a Claude Code inbox.
//!
//! A Claude Code session with
//! cross-session messaging binds an inbox socket and gives its commands the
//! socket's path and a per-session token in its environment
//! (CLAUDE_CODE_MESSAGING_SOCKET and CLAUDE_CODE_MESSAGING_TOKEN). builds keeps
//! the inbox of a session it tells something later: the session that recorded
//! a design change (lists/design_session.rs), the design session that listens
//! for publish requests (lists/publish.rs) and the judge (review/judge.rs).
//!
//! The token is a per-session secret. It lives in the store's tables and in
//! the `Messaging` that carries it to a send. No type here serializes it,
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

use std::io::{ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::ack::agent::ThreadId;
use crate::db::Sql;
use crate::fail;
use crate::fail::Result;

mod codex;

/// How long a write to the socket may wait. A session that is busy still
/// reads its inbox between its tool calls, so a send that stalls is a fault.
const SEND_WAIT: Duration = Duration::from_secs(10);

/// The destination a session registered for messages.
#[derive(Clone, PartialEq, Eq)]
pub enum Messaging {
    Claude(ClaudeInbox),
    Codex(ThreadId),
}

#[derive(Clone, PartialEq, Eq)]
pub struct ClaudeInbox {
    socket: String,
    token: Option<String>,
}

impl std::fmt::Debug for Messaging {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (runtime, address, _) = self.stored();
        f.debug_struct("Messaging")
            .field("runtime", &runtime)
            .field("address", &address)
            .finish()
    }
}

impl Messaging {
    pub fn new(socket: &str, token: Option<&str>) -> Result<Messaging> {
        if !socket.starts_with('/') {
            fail!("CLAUDE_CODE_MESSAGING_SOCKET is not an absolute path");
        }
        Ok(Messaging::Claude(ClaudeInbox {
            socket: socket.to_string(),
            token: token.filter(|t| !t.trim().is_empty()).map(str::to_string),
        }))
    }

    /// The runtime and address stored by the caller's table, with a token
    /// only for a Claude inbox that uses one.
    pub fn stored(&self) -> (&str, &str, Option<&str>) {
        match self {
            Self::Claude(inbox) => ("claude", &inbox.socket, inbox.token.as_deref()),
            Self::Codex(thread) => ("codex", thread.id(), None),
        }
    }

    pub fn restore(runtime: &str, address: &str, token: Option<&str>) -> Result<Self> {
        match (runtime, token) {
            ("claude", token) => Self::new(address, token),
            ("codex", None) => Ok(Self::Codex(ThreadId::parse(address)?)),
            _ => {
                fail!("builds could not read a stored session destination for runtime {runtime:?}")
            }
        }
    }

    /// The address of the caller's own thread or its coordinator.
    pub fn for_session(
        session: &crate::ack::agent::Session,
        to: Recipient,
    ) -> Result<Option<Self>> {
        Ok(Destinations::for_session(session)?.map(|destinations| destinations.at(to)))
    }
}

/// The two destinations of a recording agent. Claude uses one inbox for
/// both, while a Codex worker and its coordinator have separate threads.
pub enum Destinations {
    Shared(Messaging),
    Codex {
        thread: ThreadId,
        coordinator: ThreadId,
    },
}

impl Destinations {
    /// The destinations of the session that runs this command: its Codex
    /// threads, or the inbox its Claude Code shell names, if any.
    pub fn for_session(session: &crate::ack::agent::Session) -> Result<Option<Self>> {
        use crate::ack::agent::{Identity, Session};
        match session {
            Session::Codex { .. } => match session.identity()? {
                Some(Identity::Codex { session, thread }) => Ok(Some(Self::Codex {
                    thread,
                    coordinator: session,
                })),
                _ => Ok(None),
            },
            Session::Claude { inbox, .. } => match inbox {
                Some(Ok(inbox)) => Ok(Some(Self::Shared(inbox.clone()))),
                Some(Err(why)) => fail!("{why}"),
                None => Ok(None),
            },
            Session::InvalidClaude => {
                session.claude()?;
                Ok(None)
            }
            Session::Terminal => Ok(None),
        }
    }

    pub fn at(&self, recipient: Recipient) -> Messaging {
        match self {
            Self::Shared(inbox) => inbox.clone(),
            Self::Codex {
                thread,
                coordinator,
            } => Messaging::Codex(match recipient {
                Recipient::ThisThread => thread.clone(),
                Recipient::Coordinator => coordinator.clone(),
            }),
        }
    }
}

/// The three endpoint columns held by a domain's registration table.
pub struct StoredEndpoint(String, String, Option<String>);

impl StoredEndpoint {
    pub fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self(row.get(0)?, row.get(1)?, row.get(2)?))
    }

    pub fn restore(self) -> Result<Messaging> {
        Messaging::restore(&self.0, &self.1, self.2.as_deref())
    }
}

/// Carries the original Claude destination into the registering domain's
/// new table. The domain names its key and the rest of the fields it owns.
pub fn move_legacy_destination(
    db: &rusqlite::Connection,
    table: &str,
    create: &str,
    key: &str,
    tail: &str,
) -> Result<()> {
    crate::db::remake_table(
        db,
        table,
        create,
        |db| {
            db.execute_batch(&format!("INSERT INTO {table} ({key}, runtime, address, token{tail}) SELECT {key}, 'claude', socket, token{tail} FROM {table}_old;")).sql()
        },
        &[],
    )
}

#[derive(Clone, Copy)]
pub enum Recipient {
    ThisThread,
    Coordinator,
}

/// Whether a message reached the inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Delivered,
    /// Codex accepted the message but could not wake the thread.
    Queued,
    /// Nothing listens on the socket: the session ended.
    Gone,
}

impl Sent {
    pub fn resume_notice(self) -> &'static str {
        match self {
            Self::Queued => "Codex queued the message but could not resume the receiving thread. ",
            Self::Delivered | Self::Gone => "",
        }
    }
}

/// Programs and state that reach registered sessions. The processes are read
/// once, at the first question about running Claude Code sessions, and every
/// clone and every question after it answers from that reading, so one
/// health pass or one command judges the sessions from one list.
#[derive(Clone, Debug)]
pub struct Sessions {
    pub claude: std::path::PathBuf,
    pub codex: std::path::PathBuf,
    processes:
        std::sync::Arc<std::sync::OnceLock<std::result::Result<crate::procs::Processes, String>>>,
}

impl Sessions {
    pub fn new(claude: std::path::PathBuf, codex: std::path::PathBuf) -> Sessions {
        Sessions {
            claude,
            codex,
            processes: Default::default(),
        }
    }

    /// The Claude Code sessions that run, from this value's one reading of
    /// the processes (`claude_sessions::running`).
    pub fn running_claude(
        &self,
    ) -> std::result::Result<Vec<crate::claude_sessions::Running>, String> {
        let processes = self
            .processes
            .get_or_init(|| crate::procs::Processes::read().map_err(|error| error.text()))
            .as_ref()
            .map_err(String::clone)?;
        crate::claude_sessions::running(&self.claude, processes)
    }

    pub fn send(&self, to: &Messaging, text: &str) -> Result<Sent> {
        match to {
            Messaging::Claude(inbox) => send_claude(inbox, text),
            Messaging::Codex(thread) => codex::queue(thread, &self.codex, text),
        }
    }

    pub fn codex_can_resume(&self, thread: &ThreadId) -> Result<bool> {
        codex::exists(thread, &self.codex)
    }
}

fn send_claude(to: &ClaudeInbox, text: &str) -> Result<Sent> {
    // A test never reaches a live session: it sends only to a socket in a
    // test's scratch folder.
    #[cfg(test)]
    if !crate::testing::in_scratch(std::path::Path::new(&to.socket)) {
        fail!(
            "a test sent to {}, which is in no test's scratch folder: a test never reaches a live session",
            to.socket
        );
    }
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

#[cfg(test)]
#[path = "inbox_tests.rs"]
mod tests;
