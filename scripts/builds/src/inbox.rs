//! A Claude Code session's inbox, and sending it a message. A session with
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

use crate::fail;
use crate::fail::Result;

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

    /// The inbox's socket, which the store keeps.
    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// The inbox's token, which only the store keeps.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
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

#[cfg(test)]
#[path = "inbox_tests.rs"]
mod tests;
