//! The dashboard's live view of a board: the board as the files of one
//! worktree of the repo that holds a project's design files draw it now,
//! uncommitted edits included, beside the board as origin/main draws it
//! (decision #341). The page redraws both each time a file in the worktree's
//! design folders changes, so a design round needs no commit.
//!
//! The page follows a stream of server-sent events: one `drawing` event on
//! connect, and one after each batch of changes that FSEvents reports in the
//! folders that hold the worktree's design places and shared sources
//! (`Globs::folders`). Each event gives each side's frame size, or why the
//! side has no drawing. The frames load the board's own pages, drawn on each
//! request the way the review page's frames draw them
//! (`design_render::render_board`): the worktree's from its files, with the
//! bundles that its build-bundle.py builds from its design system's sources
//! on each draw, and origin/main's from that commit.
//!
//! The live view shows files that no commit holds, so the dashboard serves it
//! only on this Mac: to a loopback peer, at 127.0.0.1 or localhost
//! (`http::on_this_mac`).
//!
//!     /design/<project>/live/<worktree>/<board>          the page
//!     /design/<project>/live/<worktree>/<board>/events   its `drawing` events
//!     /design/<project>/live/<worktree>/<board>/<theme>/now and /worktree
//!                   the board in the light or dark theme, as origin/main or
//!                   the worktree's files draw it; the page's frames load
//!                   these
//!
//! <worktree> is `main` for the repo's main checkout, or the folder name of
//! one of its worktrees (`side-4`).

use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use boards::{MOCKS, Source, board_name};
use serde_json::{Value, json};

use super::fsevents::{Event, Stream, flags};
use super::http::{drawn_page, respond};
use super::model::Model;
use crate::checkout::{Checkout, worktrees_folder};
use crate::design_render::{
    FRAMES_CSS, FRAMES_SCRIPT, NOW, board_size, fill, render_board, script_json,
};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::Theme;
use crate::lists::design_paths::Globs;

const LIVE_PAGE: &str = include_str!("live_view.html");
/// How long FSEvents holds changes back to report them as one batch, in
/// seconds: a save that writes several files redraws the board once.
const LATENCY: f64 = 0.1;
/// How often an events stream with no change to send writes a comment, so
/// that the write fails, and the stream and its FSEvents stream end, once
/// the page has gone.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// The board a live view address names: the project, the worktree and the
/// board, as the address gives them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target<'a> {
    pub project: &'a str,
    pub worktree: &'a str,
    pub board: &'a str,
}

/// Which drawing of the board a frame loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The board as it stands on origin/main.
    Now,
    /// The board as the worktree's files draw it.
    Worktree,
}

/// What a live view address asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route<'a> {
    Page(Target<'a>),
    Events(Target<'a>),
    Drawing(Target<'a>, Theme, Side),
}

impl<'a> Route<'a> {
    fn target(&self) -> Target<'a> {
        match *self {
            Route::Page(target) | Route::Events(target) | Route::Drawing(target, _, _) => target,
        }
    }
}

/// The live view route of <path>, or None when it is no live view address.
pub fn route(path: &str) -> Option<Route<'_>> {
    let parts: Vec<&str> = path.strip_prefix("/design/")?.split('/').collect();
    let target = |project, worktree, board| Target {
        project,
        worktree,
        board,
    };
    match parts.as_slice() {
        [project, "live", worktree, board] => Some(Route::Page(target(project, worktree, board))),
        [project, "live", worktree, board, "events"] => {
            Some(Route::Events(target(project, worktree, board)))
        }
        [project, "live", worktree, board, theme, side] => Some(Route::Drawing(
            target(project, worktree, board),
            Theme::of_word(theme).ok()?,
            match *side {
                "now" => Side::Now,
                "worktree" => Side::Worktree,
                _ => return None,
            },
        )),
        _ => None,
    }
}

/// A worktree the live view draws from: its top, and the folders in it that
/// hold its design places and shared sources, which the view follows.
#[derive(Debug, PartialEq, Eq)]
pub struct Worktree {
    pub top: PathBuf,
    pub folders: Vec<PathBuf>,
}

/// Worktree <name> of the repo whose main checkout is <main> and whose
/// design files <globs> name: `main` for the main checkout, or a folder in
/// the repo's worktrees folder (`checkout::worktrees_folder`). Fails for any
/// other name, and for a folder that is no worktree of that repo.
pub fn worktree_of(main: &Path, globs: &Globs, name: &str) -> Result<Worktree> {
    let folder = if name == "main" {
        main.to_path_buf()
    } else if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        fail!("{name:?} names no worktree: give main or a worktree's folder name, such as side-4")
    } else {
        match worktrees_folder(main) {
            Some(folder) => folder.join(name),
            None => fail!("{} has no folder for worktrees", main.display()),
        }
    };
    let top = match std::fs::canonicalize(&folder) {
        Ok(top) => top,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fail!("{} has no worktree {name}", main.display())
        }
        Err(error) => fail!("reading {}: {error}", folder.display()),
    };
    let main = std::fs::canonicalize(main).context(|| format!("reading {}", main.display()))?;
    match Checkout::at(&top)? {
        Some(checkout) if checkout.top == top && checkout.main == main => {}
        _ => fail!("{} is no worktree of {}", top.display(), main.display()),
    }
    let folders = globs
        .folders()
        .into_iter()
        .map(|folder| top.join(folder))
        .collect();
    Ok(Worktree { top, folders })
}

/// Where <side> of the board is read from in <worktree>.
fn source(worktree: &Worktree, side: Side) -> Source<'_> {
    match side {
        Side::Now => Source::Commit {
            repo: &worktree.top,
            rev: NOW,
        },
        Side::Worktree => Source::Dir(&worktree.top),
    }
}

/// The `drawing` event's data for board <board> of <worktree>: for each side
/// (`now` and `worktree`), its frame's size (`w` and `h`), `absent` for a
/// board that origin/main has no file of, or the `error` that stops it.
pub fn drawing(worktree: &Worktree, board: &str) -> Value {
    let side = |side: Side| match board_size(&source(worktree, side), board) {
        Ok(Some((w, h))) => json!({ "w": w, "h": h }),
        Ok(None) if side == Side::Now => json!({ "absent": true }),
        Ok(None) => json!({
            "error": format!("{board} is no file in {}", Source::Dir(&worktree.top).label(MOCKS))
        }),
        Err(error) => json!({ "error": error.text() }),
    };
    json!({ "now": side(Side::Now), "worktree": side(Side::Worktree) })
}

/// The live view's page of board <target> of <worktree>.
pub fn page(target: Target, worktree: &Worktree) -> Result<String> {
    board_name(target.board)?;
    let Target {
        project,
        worktree: name,
        board,
    } = target;
    let data = json!({
        "worktree": name,
        "top": worktree.top.display().to_string(),
        "board": board,
        "base": format!("/design/{project}/live/{name}/{board}"),
    });
    Ok(fill(
        LIVE_PAGE,
        &[
            ("FRAMES_CSS", FRAMES_CSS),
            ("FRAMES", FRAMES_SCRIPT),
            ("DATA", &script_json(&data)),
        ],
    ))
}

/// Answers a request for <route> on <stream>: 404 when its project has no
/// design files, and the error that stops it when its worktree or board
/// can't be read.
pub fn serve(mut stream: TcpStream, model: &Model, route: Route) -> std::io::Result<()> {
    let target = route.target();
    let Some(found) = model.design_repo(target.project) else {
        return respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            format!("project {} has no design files\n", target.project).as_bytes(),
        );
    };
    let worktree = found.and_then(|(main, globs)| worktree_of(&main, &globs, target.worktree));
    let answer = match (route, worktree) {
        (Route::Events(_), Ok(worktree)) => return events(stream, &worktree, target.board),
        (_, Err(error)) => Err(error),
        (Route::Page(_), Ok(worktree)) => page(target, &worktree),
        (Route::Drawing(_, theme, side), Ok(worktree)) => render_board(
            source(&worktree, side),
            target.board,
            theme,
            &model.places.design_system_cache(),
        ),
    };
    drawn_page(&mut stream, Some(answer), b"")
}

/// Does a batch of <events> change a file? A copy that reads a file, such as
/// the copy of the design system that each drawing of the worktree's side
/// builds its bundles in, clones the file on APFS, and FSEvents reports the
/// clone in the folder it read. An event that says only that changes
/// nothing, and a redraw for it would draw again for its own copy.
fn changes_files(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| event.flags & !flags::ITEM_KINDS != flags::ITEM_CLONED)
}

/// Streams `drawing` events of board <board> of <worktree> until the page
/// goes: one now, and one after each batch of changes in the worktree's
/// design folders.
fn events(mut stream: TcpStream, worktree: &Worktree, board: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n"
    )?;
    let (changed, heard) = channel();
    let folders: Vec<&Path> = worktree.folders.iter().map(PathBuf::as_path).collect();
    // The stream follows before the first drawing, so a change made while
    // that drawing is read sends another.
    let _following = Stream::start(
        &folders,
        &[],
        LATENCY,
        Box::new(move |events| {
            if changes_files(&events) {
                let _ = changed.send(());
            }
        }),
    );
    loop {
        write!(
            stream,
            "event: drawing\ndata: {}\n\n",
            drawing(worktree, board)
        )?;
        stream.flush()?;
        loop {
            match heard.recv_timeout(KEEPALIVE) {
                Ok(()) => break,
                Err(RecvTimeoutError::Timeout) => {
                    stream.write_all(b": following\n\n")?;
                    stream.flush()?;
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
        // One drawing answers every batch heard so far.
        while heard.try_recv().is_ok() {}
    }
}

#[cfg(test)]
#[path = "live_view_tests.rs"]
pub(super) mod tests;
