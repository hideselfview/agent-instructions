//! Asking the daemon for sizes. A command never depends on it: with no
//! daemon, or one that doesn't answer in time, the command measures itself.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Question, Reply};
use crate::health::Health;
use crate::places::Places;
use crate::text::note;
use crate::usage::Measure;

/// How long a command waits for the daemon's answer before measuring
/// itself: a fresh answer waits for FSEvents to deliver the last writes,
/// about a second under load, and is still far less than a walk of a build
/// folder.
const PATIENCE: Duration = Duration::from_secs(5);

/// The daemon's answer for each of <paths> (None for a path it can't answer
/// for), or None when no daemon answers. <fresh>: see `Question`.
pub fn measure(places: &Places, paths: &[PathBuf], fresh: bool) -> Option<Vec<Option<Measure>>> {
    match ask(
        places,
        &Question::Measure {
            paths: paths.to_vec(),
            fresh,
        },
    )? {
        Reply::Measure(answers) if answers.len() == paths.len() => Some(answers),
        Reply::Measure(answers) => {
            note(format!(
                "the daemon at {} answered {} of {} paths; measuring here",
                places.daemon_socket().display(),
                answers.len(),
                paths.len()
            ));
            None
        }
        other => {
            unexpected(places, &other);
            None
        }
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
        Reply::Measure(_) => "sizes".to_string(),
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
    match ask_within(&socket, question, PATIENCE) {
        Ok(reply) => reply,
        Err(message) => {
            note(message);
            None
        }
    }
}

/// Asks the daemon listening on <socket> one question and waits <patience>
/// for each step. Ok(None): no daemon there. Err: what went wrong.
fn ask_within(
    socket: &Path,
    question: &Question,
    patience: Duration,
) -> Result<Option<Reply>, String> {
    if !socket.exists() {
        return Ok(None);
    }
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        // A socket a daemon left behind when it stopped.
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => return Ok(None),
        Err(error) => {
            return Err(format!(
                "asking the daemon at {}: {error}; measuring here",
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
