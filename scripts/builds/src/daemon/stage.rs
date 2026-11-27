//! `builds dashboard stage --fixtures <folder> <shot>`: serves the dashboard's
//! routes for one shot on 127.0.0.1, with the answers of fixture `<shot>.json`
//! in place of the daemon's data (pages.rs), and ends when its stdin closes.
//! The shots program starts it for each shot of the dashboard, reads the page's
//! address from the first line of its stdout and loads it in Chrome.
//!
//! It touches no store, checkout or build volume, so it runs on any host and
//! beside a running dashboard.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use super::http::serve_staged;
use super::pages::{Fixture, Pages};
use crate::fail;
use crate::fail::{Context, Result};

const USAGE: &str = "usage: builds dashboard stage --fixtures <folder> <shot>";

/// Reads the fixture the arguments name, prints the page's address, and serves
/// until stdin closes.
pub fn cmd_stage(args: &[String]) -> Result<()> {
    let (folder, shot) = stage_args(args)?;
    let fixture = Fixture::load(&folder, &shot)?;
    let listener = bind()?;
    crate::output::answer_line(address(port_of(&listener)?, &fixture));
    crate::output::answer_handle()
        .flush()
        .context(|| "writing the page's address".to_string())?;
    let names: Arc<[String]> = names_of(&listener)?.into();
    let pages: Arc<dyn Pages> = Arc::new(fixture);
    std::thread::Builder::new()
        .name("builds-staged-page".into())
        .spawn(move || serve_staged(listener, names, pages))
        .context(|| "starting a thread".to_string())?;
    let mut sink = Vec::new();
    std::io::stdin()
        .read_to_end(&mut sink)
        .context(|| "reading stdin".to_string())?;
    Ok(())
}

/// The fixtures' folder and the shot, from `--fixtures <folder> <shot>` in
/// either order.
fn stage_args(args: &[String]) -> Result<(PathBuf, String)> {
    let mut folder = None;
    let mut shot = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--fixtures" => match rest.next() {
                Some(value) if folder.is_none() => folder = Some(PathBuf::from(value)),
                _ => fail!("{USAGE}"),
            },
            word if !word.starts_with('-') && shot.is_none() => shot = Some(word.to_string()),
            _ => fail!("{USAGE}"),
        }
    }
    match (folder, shot) {
        (Some(folder), Some(shot)) => Ok((folder, shot)),
        _ => fail!("{USAGE}"),
    }
}

/// Listens on a port the system picks.
fn bind() -> Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", 0)).context(|| "listening on 127.0.0.1".to_string())
}

/// The port <listener> listens on.
fn port_of(listener: &TcpListener) -> Result<u16> {
    Ok(listener
        .local_addr()
        .context(|| "reading the port".to_string())?
        .port())
}

/// The address of <fixture>'s page on <port>.
fn address(port: u16, fixture: &Fixture) -> String {
    format!("http://127.0.0.1:{port}{}", fixture.path())
}

/// The `host:port` names a request may give in its Host header.
fn names_of(listener: &TcpListener) -> Result<Vec<String>> {
    let port = port_of(listener)?;
    Ok(vec![
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
    ])
}

#[cfg(test)]
#[path = "stage_tests.rs"]
mod tests;
