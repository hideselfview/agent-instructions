//! The daemon: `builds dashboard` runs it in the foreground until it is
//! killed. It follows the build volume (tracker.rs) and answers commands'
//! questions about sizes on <root>/.builds-daemon/socket, one daemon per root.
//!
//! The question is one JSON line, {"measure": [<path>, ...], "fresh": bool};
//! the answer one
//! JSON line, {"answers": [{"kib": n, "newest": t} or
//! ...]}, null for a path the daemon can't answer for.

pub mod client;
pub mod fsevents;
pub mod index;
pub mod tracker;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::DASHBOARD_PORT;
use crate::text::note;
use crate::tool::Tool;
use crate::usage::Measure;
use tracker::Tracker;

#[derive(Serialize, Deserialize)]
pub struct Question {
    pub measure: Vec<PathBuf>,
    /// Count every write made before the question (a deciding command
    /// measuring after its own deletions), at the cost of waiting for
    /// FSEvents to deliver them.
    pub fresh: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Reply {
    pub answers: Vec<Option<Measure>>,
}

/// Binds the root's socket, refusing when another daemon answers on it and
/// replacing one a daemon left behind.
fn bind(tool: &Tool) -> Result<UnixListener> {
    let socket = tool.places.daemon_socket();
    let dir = tool.places.daemon_dir();
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    if socket.exists() {
        match UnixStream::connect(&socket) {
            Ok(_) => fail!("a daemon already answers on {}", socket.display()),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                std::fs::remove_file(&socket).context(|| format!("removing {}", socket.display()))?;
            }
            Err(error) => fail!("checking {}: {error}", socket.display()),
        }
    }
    UnixListener::bind(&socket).context(|| format!("listening on {}", socket.display()))
}

/// Answers one question. Paths may name the root as the commands' places
/// do (<given>, which may pass through a link) or resolved.
fn answer(tracker: &Tracker, given: &Path, stream: UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let question: Question = serde_json::from_str(&line)?;
    let paths: Vec<PathBuf> = question
        .measure
        .into_iter()
        .map(|path| match path.strip_prefix(given) {
            Ok(rest) => tracker.root.join(rest),
            Err(_) => path,
        })
        .collect();
    let reply = Reply { answers: tracker.measure(&paths, question.fresh) };
    let mut stream = stream;
    stream.write_all(serde_json::to_string(&reply)?.as_bytes())?;
    stream.write_all(b"\n")
}

/// `builds dashboard [--port <port>]`.
pub fn cmd_dashboard(tool: &Tool, args: &[String]) -> Result<()> {
    // Sentinel: the page, which comes with the daemon's HTTP server, serves on
    // this port; until then the daemon only answers on its socket.
    let mut _port = DASHBOARD_PORT;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--port" => {
                let value = rest.next().map(String::as_str).unwrap_or("");
                match value.parse() {
                    Ok(port) if value.bytes().all(|b| b.is_ascii_digit()) => _port = port,
                    _ => fail!("dashboard: --port takes a port number"),
                }
            }
            _ => fail!("usage: builds dashboard [--port <port>]"),
        }
    }
    tool.places.require_volume()?;
    let root = std::fs::canonicalize(&tool.places.root).context(|| format!("resolving {}", tool.places.root.display()))?;
    let listener = bind(tool)?;
    let tracker = Tracker::start(&root, &tool.places.daemon_dir()).context(|| format!("following {}", root.display()))?;
    note(format!("daemon for {} on {} (pid {})", root.display(), tool.places.daemon_socket().display(), std::process::id()));
    for stream in listener.incoming() {
        let stream = stream.context(|| "accepting a question".to_string())?;
        let tracker: Arc<Tracker> = tracker.clone();
        let given = tool.places.root.clone();
        std::thread::spawn(move || {
            if let Err(error) = answer(&tracker, &given, stream) {
                note(format!("answering a question: {error}"));
            }
        });
    }
    Ok(())
}
