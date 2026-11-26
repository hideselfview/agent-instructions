//! The dashboard's scratch view of a board: the board as the files of one
//! worktree of the repo that holds a project's design files draw it now,
//! uncommitted edits included, beside the board as origin/main draws it
//! (decision #341). The page redraws both each time a file in the worktree's
//! design folders changes, so a design round needs no commit.
//!
//! A scratch board is a board that no commit holds:
//! `design/mocks/scratch.<name>.dc.html` (`SCRATCH_PREFIX`), which each
//! design repo's .gitignore leaves out, so git, and with it health, release and
//! land, never counts it as work. A design session draws versions of a board
//! on one, side by side, shows the person its scratch view, and commits only
//! the version the person picks to the real board (decision #349). A scratch
//! board is on no canvas, so canvas.json gives it no size: the page sizes its
//! frame to what the board draws, which the board's page measures
//! (`design_render/board.html`). origin/main has no file of a scratch board,
//! so its page shows the worktree's drawing alone.
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
//! The scratch view shows files that no commit holds, so the dashboard serves
//! it only on this Mac: to a loopback peer, at 127.0.0.1 or localhost
//! (`http::on_this_mac`). The Designs page lists every scratch board of the
//! project's worktrees (`scratch_list`), and a design change's review page
//! the ones in its worktree, also only on this Mac.
//!
//!     /design/<project>/scratch/<worktree>/<board>          the page
//!     /design/<project>/scratch/<worktree>/<board>/events   its `drawing` events
//!     /design/<project>/scratch/<worktree>/<board>/<theme>/now and /worktree
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

use boards::{BOARD_SUFFIX, MOCKS, Source, board_name, board_path};
use serde_json::{Value, json};

use super::fsevents::{Event, Stream, flags};
use super::http::{drawn_page, respond};
use super::model::Model;
use crate::checkout::{Checkout, worktree_list, worktrees_folder};
use crate::design_render::{
    FRAMES_CSS, FRAMES_SCRIPT, NOW, board_size, fill, render_board, script_json,
};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::Theme;
use crate::lists::design_paths::Globs;

const SCRATCH_PAGE: &str = include_str!("scratch_view.html");
/// How a scratch board's name starts: its path is
/// `design/mocks/scratch.<name>.dc.html`, the pattern that each design repo's
/// .gitignore holds.
const SCRATCH_PREFIX: &str = "scratch.";
/// How long FSEvents holds changes back to report them as one batch, in
/// seconds: a save that writes several files redraws the board once.
const LATENCY: f64 = 0.1;
/// How often an events stream with no change to send writes a comment, so
/// that the write fails, and the stream and its FSEvents stream end, once
/// the page has gone.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// The board a scratch view address names: the project, the worktree and the
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

/// What a scratch view address asks for.
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

/// The scratch view route of <path>, or None when it is no scratch view
/// address.
pub fn route(path: &str) -> Option<Route<'_>> {
    let parts: Vec<&str> = path.strip_prefix("/design/")?.split('/').collect();
    let target = |project, worktree, board| Target {
        project,
        worktree,
        board,
    };
    match parts.as_slice() {
        [project, "scratch", worktree, board] => {
            Some(Route::Page(target(project, worktree, board)))
        }
        [project, "scratch", worktree, board, "events"] => {
            Some(Route::Events(target(project, worktree, board)))
        }
        [project, "scratch", worktree, board, theme, side] => Some(Route::Drawing(
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

/// A worktree the scratch view draws from: its top, and the folders in it that
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

/// Is the board named <name> a scratch board?
pub fn is_scratch(name: &str) -> bool {
    name.starts_with(SCRATCH_PREFIX)
}

/// The frame of board <board> as <from> draws it: the size canvas.json gives
/// it (`w` and `h`), or for a scratch board, which is on no canvas, `fit`:
/// the page sizes the frame to what the board draws. None when the board is
/// no file there.
fn frame(from: &Source, board: &str) -> Result<Option<Value>> {
    let name = board_name(board)?;
    if !is_scratch(&name) {
        return Ok(board_size(from, board)?.map(|(w, h)| json!({ "w": w, "h": h })));
    }
    from.check()?;
    Ok(from
        .has(&board_path(&name))?
        .then(|| json!({ "fit": true })))
}

/// The `drawing` event's data for board <board> of <worktree>: for each side
/// (`now` and `worktree`), its frame (`frame`), `absent` for a board that
/// origin/main has no file of, or the `error` that stops it.
pub fn drawing(worktree: &Worktree, board: &str) -> Value {
    let side = |side: Side| match frame(&source(worktree, side), board) {
        Ok(Some(frame)) => frame,
        Ok(None) if side == Side::Now => json!({ "absent": true }),
        Ok(None) => json!({
            "error": format!("{board} is no file in {}", Source::Dir(&worktree.top).label(MOCKS))
        }),
        Err(error) => json!({ "error": error.text() }),
    };
    json!({ "now": side(Side::Now), "worktree": side(Side::Worktree) })
}

/// The scratch view's page of board <target> of <worktree>.
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
        "base": format!("/design/{project}/scratch/{name}/{board}"),
    });
    Ok(fill(
        SCRATCH_PAGE,
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
            &model.places.design_dirs(),
        ),
    };
    drawn_page(&mut stream, Some(answer), b"")
}

/// The worktrees of the repo whose main checkout is <main> that a scratch
/// view address names, each with its name there (`worktree_of`): `main` for
/// the main checkout, then each worktree in the repo's worktrees folder, by
/// name. git lists the worktrees.
pub fn named_worktrees(main: &Path) -> Result<Vec<(String, PathBuf)>> {
    let main_top = canonical(main)?;
    let folder = match worktrees_folder(main) {
        None => None,
        Some(folder) => match std::fs::canonicalize(&folder) {
            Ok(folder) => Some(folder),
            // A repo with no worktrees folder has no worktree in it.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => fail!("reading {}: {error}", folder.display()),
        },
    };
    let mut named = vec![("main".to_string(), main_top.clone())];
    let mut sides = Vec::new();
    for listed in worktree_list(main)? {
        let top = match std::fs::canonicalize(&listed.top) {
            Ok(top) => top,
            // git lists a worktree whose folder someone deleted until
            // `git worktree prune` forgets it; it holds no files.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "builds: the scratch list leaves out {}, a worktree that git lists and whose folder is gone",
                    listed.top.display()
                );
                continue;
            }
            Err(error) => fail!("reading {}: {error}", listed.top.display()),
        };
        if top != main_top
            && folder.is_some()
            && top.parent() == folder.as_deref()
            && let Some(name) = top.file_name()
        {
            sides.push((name.to_string_lossy().into_owned(), top));
        }
    }
    sides.sort();
    named.extend(sides);
    Ok(named)
}

/// <path> with every link resolved.
fn canonical(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).context(|| format!("reading {}", path.display()))
}

/// A file in a worktree's design/mocks whose name says it is a scratch
/// board (`scratch.*.dc.html`): its file name, and the board's name, or why
/// the file's name names no board.
pub struct ScratchFile {
    pub file: String,
    pub board: Result<String>,
}

/// The scratch board files in the worktree whose top is <top>, in file name
/// order.
pub fn scratch_files_in(top: &Path) -> Result<Vec<ScratchFile>> {
    let mocks = top.join(MOCKS);
    let entries = match std::fs::read_dir(&mocks) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", mocks.display()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", mocks.display()))?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if let Some(name) = file.strip_suffix(BOARD_SUFFIX)
            && is_scratch(name)
        {
            let board = board_name(name).map_err(Into::into);
            files.push(ScratchFile { file, board });
        }
    }
    files.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(files)
}

/// The scratch boards a page lists, or why they can't be listed.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Scratch {
    Failed { error: String },
    Boards { boards: Vec<ScratchBoard> },
}

/// One scratch board of a worktree: its name and the address of its scratch
/// view, or its file's name and why that names no board.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(untagged, deny_unknown_fields)]
pub enum ScratchBoard {
    Unnamed {
        worktree: String,
        file: String,
        error: String,
    },
    Named {
        worktree: String,
        board: String,
        link: String,
    },
}

/// The scratch boards of <worktrees> (`named_worktrees`), as the Designs page
/// and a review page list them, by worktree then file: each with the
/// worktree's name, and the board's name and the address of its scratch view
/// in project <project> (`link`), or its file's name and why that names no
/// board (`error`).
fn list_of(project: &str, worktrees: &[(String, PathBuf)]) -> Result<Vec<ScratchBoard>> {
    let mut list = Vec::new();
    for (name, top) in worktrees {
        for file in scratch_files_in(top)? {
            list.push(match file.board {
                Ok(board) => ScratchBoard::Named {
                    worktree: name.clone(),
                    link: format!("/design/{project}/scratch/{name}/{board}"),
                    board,
                },
                Err(error) => ScratchBoard::Unnamed {
                    worktree: name.clone(),
                    file: file.file,
                    error: error.text(),
                },
            });
        }
    }
    Ok(list)
}

/// The scratch boards of every worktree of the repo whose main checkout is
/// <main>, as the Designs page lists them (`list_of`).
pub fn scratch_list(project: &str, main: &Path) -> Result<Vec<ScratchBoard>> {
    list_of(project, &named_worktrees(main)?)
}

/// The scratch boards of the worktree whose folder is <top>, as the review
/// page of a design change drawn there lists them (`list_of`). Fails when
/// <top> is no worktree of the repo whose main checkout is <main> that a
/// scratch view address names.
pub fn scratch_list_of(project: &str, main: &Path, top: &Path) -> Result<Vec<ScratchBoard>> {
    let at = canonical(top)?;
    let worktrees: Vec<(String, PathBuf)> = named_worktrees(main)?
        .into_iter()
        .filter(|(_, folder)| *folder == at)
        .collect();
    if worktrees.is_empty() {
        fail!(
            "{} is no worktree of {} that a scratch view draws from",
            top.display(),
            main.display()
        );
    }
    list_of(project, &worktrees)
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
#[path = "scratch_view_tests.rs"]
pub(super) mod tests;
