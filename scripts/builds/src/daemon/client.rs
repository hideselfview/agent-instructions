//! Asking the daemon for sizes. A command never depends on it: with no
//! daemon, or one that doesn't answer in time, the command measures itself.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use super::{Question, Reply};
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
    let socket = places.daemon_socket();
    if !socket.exists() {
        return None;
    }
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        // A socket a daemon left behind when it stopped.
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => return None,
        Err(error) => {
            note(format!("asking the daemon at {}: {error}; measuring here", socket.display()));
            return None;
        }
    };
    let asked = (|| -> std::io::Result<Reply> {
        stream.set_read_timeout(Some(PATIENCE))?;
        stream.set_write_timeout(Some(PATIENCE))?;
        let mut writer = stream.try_clone()?;
        writer.write_all(serde_json::to_string(&Question { measure: paths.to_vec(), fresh })?.as_bytes())?;
        writer.write_all(b"\n")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    match asked {
        Ok(reply) if reply.answers.len() == paths.len() => Some(reply.answers),
        Ok(reply) => {
            note(format!(
                "the daemon at {} answered {} of {} paths; measuring here",
                socket.display(),
                reply.answers.len(),
                paths.len()
            ));
            None
        }
        Err(error) => {
            note(format!("the daemon at {} did not answer: {error}; measuring here", socket.display()));
            None
        }
    }
}
