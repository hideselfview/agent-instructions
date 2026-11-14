//! The daemon: `builds dashboard` runs it in the foreground until it is
//! killed. It follows the build volume (tracker.rs) and every checkout
//! (model.rs), serves the page (http.rs) on 127.0.0.1, and answers commands'
//! questions on <root>/.builds-daemon/socket, one daemon per root.
//!
//! A question is one JSON line, a `Question`; the answer one JSON line, a
//! `Reply`.

pub mod client;
pub mod fsevents;
pub mod http;
pub mod index;
pub mod lists;
pub mod model;
pub mod runs;
pub mod tracker;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Result};
use crate::health::Health;
use crate::places::DASHBOARD_PORT;
use crate::text::note;
use crate::tool::Tool;
use crate::usage::Measure;
use model::Model;
use runs::Runs;
use tracker::Tracker;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Question {
    /// Each path's size and newest write. <fresh>: count every write made
    /// before the question (a deciding command measuring after its own
    /// deletions), at the cost of waiting for FSEvents to deliver them.
    Measure { paths: Vec<PathBuf>, fresh: bool },
    /// Health, judged now.
    Health,
}

impl Question {
    /// What the question asks for, for messages naming it.
    fn about(&self) -> &'static str {
        match self {
            Question::Measure { .. } => "sizes",
            Question::Health => "health",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    /// Per path, None for one the daemon can't answer for.
    Measure(Vec<Option<Measure>>),
    Health(Box<Health>),
    Failed(String),
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
                std::fs::remove_file(&socket)
                    .context(|| format!("removing {}", socket.display()))?;
            }
            Err(error) => fail!("checking {}: {error}", socket.display()),
        }
    }
    UnixListener::bind(&socket).context(|| format!("listening on {}", socket.display()))
}

/// Answers one question. Paths may name the root as the commands' places
/// do (<given>, which may pass through a link) or resolved.
fn answer(model: &Model, given: &Path, stream: UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let reply = match serde_json::from_str(&line)? {
        Question::Measure { paths, fresh } => {
            let paths: Vec<PathBuf> = paths
                .into_iter()
                .map(|path| match path.strip_prefix(given) {
                    Ok(rest) => model.tracker.root.join(rest),
                    Err(_) => path,
                })
                .collect();
            Reply::Measure(model.tracker.measure(&paths, fresh))
        }
        Question::Health => match model.sample_now() {
            Ok(health) => Reply::Health(Box::new(health)),
            Err(fail::Fail::Message(message)) => Reply::Failed(message),
            Err(fail::Fail::Exit(code)) => Reply::Failed(format!("exit {code}")),
        },
    };
    let mut stream = stream;
    stream.write_all(serde_json::to_string(&reply)?.as_bytes())?;
    stream.write_all(b"\n")
}

/// `builds dashboard [--port <port>]`.
pub fn cmd_dashboard(tool: &Tool, args: &[String]) -> Result<()> {
    let mut port = DASHBOARD_PORT;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--port" => {
                let value = rest.next().map(String::as_str).unwrap_or("");
                match value.parse() {
                    Ok(number) if value.bytes().all(|b| b.is_ascii_digit()) => port = number,
                    _ => fail!("dashboard: --port takes a port number"),
                }
            }
            _ => fail!("usage: builds dashboard [--port <port>]"),
        }
    }
    tool.places.require_volume()?;
    let root = std::fs::canonicalize(&tool.places.root)
        .context(|| format!("resolving {}", tool.places.root.display()))?;
    let web = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(web) => web,
        Err(error) => fail!("dashboard: can't listen on 127.0.0.1:{port}: {error}"),
    };
    let socket = bind(tool)?;
    let tracker = Tracker::start(&root, &tool.places.daemon_dir())
        .context(|| format!("following {}", root.display()))?;
    let exe = std::env::current_exe().context(|| "finding this program".to_string())?;
    let runs = Runs::start(
        tool.places.clone(),
        vec![exe.to_string_lossy().into_owned()],
    )?;
    let model = Model::start(tool.places.clone(), tool.rules.clone(), tracker, runs);
    let questions = model.clone();
    let given = tool.places.root.clone();
    std::thread::Builder::new()
        .name("builds-questions".into())
        .spawn(move || {
            for stream in socket.incoming() {
                let Ok(stream) = stream else { continue };
                let model = questions.clone();
                let given = given.clone();
                std::thread::spawn(move || {
                    if let Err(error) = answer(&model, &given, stream) {
                        note(format!("answering a question: {error}"));
                    }
                });
            }
        })
        .context(|| "starting a thread".to_string())?;
    note(format!(
        "dashboard at http://127.0.0.1:{port}/ (pid {})",
        std::process::id()
    ));
    http::serve(web, port, model);
    Ok(())
}
