//! The daemon: `builds dashboard` runs it in the foreground until it is
//! killed. It follows every checkout (model.rs), reads the sizes that commands
//! record for the build volume's folders (sizes.rs) and never watches the
//! volume, serves the page (http.rs) on 127.0.0.1, and answers commands'
//! questions on <root>/.builds-daemon/socket, one daemon per root.
//!
//! A question is one JSON line, a `Question`; the answer one JSON line, a
//! `Reply`.

pub mod app_builds;
pub mod approve;
pub mod board;
pub mod card_recorder;
pub mod cards;
pub mod client;
pub mod color_group;
pub mod component_shots;
pub mod design_cards;
pub mod designs;
pub mod effect;
pub mod fsevents;
pub mod hosts;
pub mod http;
pub mod lists;
pub mod live;
pub mod model;
pub mod review_cards;
pub mod runs;
pub mod scratch_view;
#[cfg(target_os = "macos")]
pub mod vnode;
#[cfg(target_os = "linux")]
#[path = "vnode_linux.rs"]
pub mod vnode;

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
use model::Model;
use runs::Runs;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Question {
    /// A command wrote these build roots: it measured each one's folders and
    /// recorded the sizes (sizes.rs), or it made or removed the root.
    Wrote { roots: Vec<PathBuf> },
    /// Health, as the daemon's last reading of the processes judged it.
    Health,
}

impl Question {
    /// What the question asks for, for messages naming it.
    fn about(&self) -> &'static str {
        match self {
            Question::Wrote { .. } => "what a command wrote",
            Question::Health => "health",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    /// The daemon marked what the command wrote to read again.
    Done,
    Health(Box<Health>),
    Failed(String),
}

/// Claims the root for this daemon and binds its socket. The claim is a lock
/// on the lock file (live.rs); a daemon that holds it is alive, so bind
/// refuses. Holding it, no other daemon owns the socket file, so one left
/// behind is removed. The caller keeps the claim as long as the daemon runs.
fn bind(tool: &Tool) -> Result<(UnixListener, live::Claim)> {
    let socket = tool.places.daemon_socket();
    let dir = tool.places.daemon_dir();
    let lock = tool.places.daemon_lock();
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let Some(claim) = live::claim(&lock).context(|| format!("locking {}", lock.display()))? else {
        fail!(
            "a daemon already runs for {}: it holds {}",
            dir.display(),
            lock.display()
        );
    };
    if socket.exists() {
        std::fs::remove_file(&socket).context(|| format!("removing {}", socket.display()))?;
    }
    let listener =
        UnixListener::bind(&socket).context(|| format!("listening on {}", socket.display()))?;
    Ok((listener, claim))
}

/// Points the daemon's stderr at <log> (appending) unless stderr is a
/// terminal: a daemon started in the background has no terminal, and its
/// notes (FSEvents losing events, a rescan of the volume, a mark that took
/// seconds) are what says why a command's question went unanswered.
fn log_to_file(log: &Path) -> Result<()> {
    use std::os::fd::AsRawFd;
    if unsafe { libc::isatty(2) } == 1 {
        return Ok(());
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .context(|| format!("opening {}", log.display()))?;
    if unsafe { libc::dup2(file.as_raw_fd(), 2) } < 0 {
        fail!(
            "pointing the daemon's notes at {}: {}",
            log.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

/// Answers one question.
fn answer(model: &Model, stream: UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let reply = match serde_json::from_str(&line)? {
        Question::Wrote { roots } => {
            model.wrote(&roots);
            Reply::Done
        }
        Question::Health => match model.health_now() {
            Ok(Some(health)) => Reply::Health(Box::new(health)),
            Ok(None) => Reply::Failed("the daemon has not read the processes yet".into()),
            Err(error) => Reply::Failed(error.text()),
        },
    };
    let mut stream = stream;
    stream.write_all(serde_json::to_string(&reply)?.as_bytes())?;
    stream.write_all(b"\n")
}

/// `builds dashboard [--port <port>] [--listen <address>]`.
pub fn cmd_dashboard(tool: &Tool, args: &[String]) -> Result<()> {
    if !cfg!(target_os = "macos") {
        fail!(
            "the dashboard follows the checkouts through macOS's FSEvents, so it runs only on a Mac"
        );
    }
    let mut port = DASHBOARD_PORT;
    let mut listen: Option<std::net::IpAddr> = None;
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
            "--listen" => {
                let value = rest.next().map(String::as_str).unwrap_or("");
                match value.parse() {
                    Ok(address) => listen = Some(address),
                    _ => fail!("dashboard: --listen takes an IP address, such as 192.168.1.20"),
                }
            }
            _ => fail!("usage: builds dashboard [--port <port>] [--listen <address>]"),
        }
    }
    tool.places.require_volume()?;
    let web = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(web) => web,
        Err(error) => fail!("dashboard: can't listen on 127.0.0.1:{port}: {error}"),
    };
    let mut names = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let also = match listen {
        Some(address) => match std::net::TcpListener::bind((address, port)) {
            Ok(also) => {
                names.push(std::net::SocketAddr::new(address, port).to_string());
                Some(also)
            }
            Err(error) => fail!("dashboard: can't listen on {address}:{port}: {error}"),
        },
        None => None,
    };
    let names: std::sync::Arc<[String]> = names.into();
    let (socket, _claim) = bind(tool)?;
    log_to_file(&tool.places.daemon_log())?;
    let exe = tool.program.clone();
    let runs = Runs::start(
        tool.places.clone(),
        vec![exe.to_string_lossy().into_owned()],
    )?;
    crate::prune_schedule::start(tool.places.clone(), exe)?;
    crate::rust_stable::start(tool.places.clone())?;
    crate::host_sample::start(tool.places.clone())?;
    let model = Model::start(tool.places.clone(), tool.rules.clone(), runs);
    card_recorder::start(model.clone())?;
    let questions = model.clone();
    std::thread::Builder::new()
        .name("builds-questions".into())
        .spawn(move || {
            for stream in socket.incoming() {
                let Ok(stream) = stream else { continue };
                let model = questions.clone();
                std::thread::spawn(move || {
                    if let Err(error) = answer(&model, stream) {
                        note(format!("answering a question: {error}"));
                    }
                });
            }
        })
        .context(|| "starting a thread".to_string())?;
    if let Some(also) = also {
        let (names, model) = (names.clone(), model.clone());
        std::thread::Builder::new()
            .name("builds-page-also".into())
            .spawn(move || http::serve(also, names, model))
            .context(|| "starting a thread".to_string())?;
    }
    let at: Vec<String> = names
        .iter()
        .filter(|name| !name.starts_with("localhost:"))
        .map(|name| format!("http://{name}/"))
        .collect();
    note(format!(
        "dashboard at {} (pid {})",
        at.join(" and "),
        std::process::id()
    ));
    http::serve(web, names, model);
    Ok(())
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
