//! Asking the daemon for health and telling it what a command wrote. A
//! command never depends on it: with no daemon, or one that doesn't answer in
//! time, the command does its work itself.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Question, Reply, live};
use crate::health::Health;
use crate::output::note;
use crate::places::Places;

/// How long a command waits for the daemon to take a note of what it wrote:
/// the daemon only marks the roots to read again.
const PATIENCE: Duration = Duration::from_secs(2);

/// Tells the daemon that <roots> were written (their folders' sizes, or the
/// roots themselves, changed) and their records are current. No daemon, or
/// one that doesn't answer, is no failure: the dashboard reads the records
/// when it starts.
pub fn wrote(places: &Places, roots: &[PathBuf]) {
    match ask(
        places,
        &Question::Wrote {
            roots: roots.to_vec(),
        },
    ) {
        Some(Reply::Done) | None => {}
        Some(other) => unexpected(places, &other),
    }
}

/// The daemon's judgement of health, or None when no daemon answers.
pub fn health(places: &Places) -> Option<Health> {
    match ask(places, &Question::Health)? {
        Reply::Health(health) => Some(*health),
        other => {
            unexpected(places, &other);
            None
        }
    }
}

fn unexpected(places: &Places, reply: &Reply) {
    let what = match reply {
        Reply::Failed(message) => message.clone(),
        Reply::Done => "done".to_string(),
        Reply::Health(_) => "health".to_string(),
    };
    note(format!(
        "the daemon at {} answered {what}; reading here",
        places.daemon_socket().display()
    ));
}

/// Asks the daemon one question; None when none answers in time.
fn ask(places: &Places, question: &Question) -> Option<Reply> {
    let socket = places.daemon_socket();
    match ask_within(&socket, &places.daemon_lock(), question, PATIENCE) {
        Ok(reply) => reply,
        Err(message) => {
            note(message);
            None
        }
    }
}

/// Asks the daemon listening on <socket> (holding <lock>) one question and waits <patience>
/// for each step. Ok(None): no daemon there. Err: what went wrong.
fn ask_within(
    socket: &Path,
    lock: &Path,
    question: &Question,
    patience: Duration,
) -> Result<Option<Reply>, String> {
    if !socket.exists() {
        return Ok(None);
    }
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        // macOS refuses a connection both to a socket a daemon left behind
        // and to a live daemon whose backlog is full: the lock tells which.
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return match live::daemon_lives(lock) {
                Ok(false) => Ok(None),
                Ok(true) => Err(format!(
                    "the daemon at {} is running and refused the connection (its queue of \
                     waiting commands is full); doing it here",
                    socket.display()
                )),
                Err(error) => Err(format!(
                    "checking whether the daemon at {} runs, by {}: {error}; doing it here",
                    socket.display(),
                    lock.display()
                )),
            };
        }
        Err(error) => {
            return Err(format!(
                "asking the daemon at {}: {error}; doing it here",
                socket.display()
            ));
        }
    };
    let asked = (|| -> std::io::Result<Reply> {
        stream.set_read_timeout(Some(patience))?;
        stream.set_write_timeout(Some(patience))?;
        let mut writer = stream.try_clone()?;
        writer.write_all(serde_json::to_string(question)?.as_bytes())?;
        writer.write_all(b"\n")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    match asked {
        Ok(reply) => Ok(Some(reply)),
        // A read or write that ran out of time ends with the operating
        // system's "Resource temporarily unavailable" (EAGAIN on macOS),
        // which says nothing of what the command was doing.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Err(format!(
                "the daemon at {} was asked for {} and waited {patience:?} without an answer; \
                 doing it here",
                socket.display(),
                question.about()
            ))
        }
        Err(error) => Err(format!(
            "the daemon at {} did not answer a question about {}: {error}; doing it here",
            socket.display(),
            question.about()
        )),
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
