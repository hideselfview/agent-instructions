//! Whether boards and component previews draw the same at two revisions.
//!
//! The checker draws the selected boards and component previews at both
//! revisions in the pinned Chrome. With no selectors, it compares every
//! drawing from either revision. It compares what each
//! draws: its picture, to within `NOISE_LEVELS` a channel, and the element
//! list the shots read from it (the boxes, roles and words), exactly. A board's frame on the canvas is part of its
//! picture, so a board resized in `canvas.json` draws differently. A board or
//! preview that exists at one revision only draws differently, and one that
//! exists at neither draws nothing at both, so it draws alike when their merge
//! base holds it; a name that no revision holds is an error. The files'
//! bytes never decide: a rebase that merges unrelated lines into a board
//! file, `canvas.json` or a component's source changes bytes and no drawing.
//!
//! A screen board takes its drawing from the picture folder (`--pictures`)
//! by its board hash (design.rs) when the folder keeps one, and draws only the
//! boards it lacks and keeps them there, as a shots run does (verify.rs). The
//! folder holds the boards of the other revision, and of every run, so a
//! comparison of a branch with its base draws only the boards whose files, or
//! whose design system, the branch changed. A board with no shot name (a
//! shared board) and a component preview are drawn every time: the folder
//! keeps no picture of either.

use crate::Checker;
use crate::cli::DrawsArgs;
use crate::design::{self, DesignSystem};
use crate::names::{Name, Platform, Previews};
use crate::pictures::Pictures;
use crate::render::Drawer;
use crate::render::serve::Tree;
use crate::render::{self, board, preview, runtime};
use crate::settings::Settings;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The most a channel of a pixel may differ between two drawings of the same
/// page. Chrome's rasterizer still puts a few pixels of a gradient or an edge
/// one level off from one run to the next while the machine is busy, though
/// `chrome::ARGS` leaves it nothing else to vary; two levels of 255 is far
/// below what anyone sees as a change.
const NOISE_LEVELS: u8 = 2;

/// The pixels per point that boards and previews draw at, the device scale of
/// a Mac shot: Chrome rounds boxes and lines to the device's pixels, so a
/// board that draws alike at this scale draws alike in a shots run.
const DEVICE_SCALE: f64 = 2.0;

/// How a comparison draws screen boards: the capture version of the checker,
/// which keys board hashes, and the picture folder it takes board pictures
/// from and keeps the ones it draws in, when builds gives one.
pub(crate) struct Store<'a> {
    pub(crate) capture_version: &'a str,
    pub(crate) pictures: Option<&'a Pictures>,
}

/// What a board or a preview draws: its picture, as the PNG Chrome made, and
/// its element list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picture {
    png: Vec<u8>,
    elements: String,
}

impl Picture {
    fn of(png: &[u8], elements: &[crate::shots::Element]) -> Result<Self, String> {
        Ok(Self {
            png: png.to_vec(),
            elements: serde_json::to_string(elements).map_err(|e| e.to_string())?,
        })
    }

    /// Whether this drawing and <other> are the same drawing: the same
    /// element list, and pictures of one size whose pixels no channel
    /// separates by more than `NOISE_LEVELS`.
    fn draws_like(&self, other: &Self) -> Result<bool, String> {
        if self.elements != other.elements {
            return Ok(false);
        }
        if self.png == other.png {
            return Ok(true);
        }
        let (a, b) = (decode(&self.png)?, decode(&other.png)?);
        Ok(a.width == b.width
            && a.height == b.height
            && a.pixels
                .iter()
                .zip(&b.pixels)
                .all(|(a, b)| a.abs_diff(*b) <= NOISE_LEVELS))
    }
}

fn decode(png: &[u8]) -> Result<crate::shots::Picture, String> {
    crate::shots::Picture::decode(png).map_err(|e| format!("a drawing's picture: {e}"))
}

/// What the drawing of each board (`board <name>`) and component preview
/// (`component <name>`) came out as; none for one the revision does not
/// draw.
pub(crate) type Drawings = BTreeMap<String, Option<Picture>>;

#[derive(Clone, Copy)]
enum Selection<'a> {
    All,
    Named {
        boards: &'a [String],
        components: &'a [String],
    },
}

/// `builds shots draws`: prints a line for each board and component preview
/// of the repo at <root> that draws the same at the two revisions of <args>,
/// taking pictures from the folder <args> names; true.
pub(crate) fn command(checker: &Checker, root: &Path, args: DrawsArgs) -> Result<bool, String> {
    let pictures = args
        .pictures
        .map(|dir| Pictures::new(dir, std::time::SystemTime::now()));
    let store = Store {
        capture_version: &checker.versions.capture,
        pictures: pictures.as_ref(),
    };
    let lines = lines(
        root,
        &args.at,
        &args.against,
        &args.boards,
        &args.components,
        checker.folders,
        &store,
    )?;
    for line in lines {
        println!("{line}");
    }
    Ok(true)
}

/// The names of the boards and component previews of the repo at <root>
/// that draw the same at <at> and at <against>: the lines `alike <what>` and
/// `differs <what>`, one for each selected drawing.
pub(crate) fn lines(
    root: &Path,
    at: &str,
    against: &str,
    boards: &[String],
    components: &[String],
    folders: &boards::Folders,
    store: &Store,
) -> Result<Vec<String>, String> {
    let selection = if boards.is_empty() && components.is_empty() {
        Selection::All
    } else {
        Selection::Named { boards, components }
    };
    let before = read(root, at, folders, selection, store)?;
    let after = read(root, against, folders, selection, store)?;
    let names: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    let mut base = None;
    names
        .into_iter()
        .map(|what| {
            let before = before.get(what).and_then(Option::as_ref);
            let after = after.get(what).and_then(Option::as_ref);
            // A name that neither revision draws draws nothing at both, so it
            // draws the same, but only when the revisions' merge base holds
            // it: a name that no revision holds is a mistake.
            if before.is_none() && after.is_none() {
                let base = match &base {
                    Some(base) => base,
                    None => base.insert(merge_base(root, at, against)?),
                };
                if !holds(root, base, what)? {
                    return Err(format!(
                        "{what} exists at neither {at} nor {against}, nor at their merge base {base}, so it names no drawing"
                    ));
                }
            }
            let alike = match (before, after) {
                (None, None) => true,
                (Some(before), Some(after)) => before.draws_like(after)?,
                _ => false,
            };
            Ok(format!(
                "{} {what}",
                if alike { "alike" } else { "differs" }
            ))
        })
        .collect()
}

/// The commit where the histories of <at> and <against> meet.
fn merge_base(root: &Path, at: &str, against: &str) -> Result<String, String> {
    boards::git_text(
        boards::git(root).args(["merge-base", at, against]),
        &format!("find the merge base of {at} and {against}"),
    )
    .map(|base| base.trim().to_string())
    .map_err(|e| e.to_string())
}

/// Does revision <rev> hold the board or component <what> (`board <name>` or
/// `component <name>`)?
fn holds(root: &Path, rev: &str, what: &str) -> Result<bool, String> {
    let path = match what.split_once(' ') {
        Some(("board", name)) => boards::board_path(name),
        Some(("component", name)) => boards::preview::preview_path(name),
        _ => return Err(format!("{what} is neither a board nor a component")),
    };
    boards::exists_in(root, rev, &path).map_err(|e| e.to_string())
}

/// How the repo at <root> draws the selection at revision <rev>.
fn read(
    root: &Path,
    rev: &str,
    folders: &boards::Folders,
    selection: Selection<'_>,
    store: &Store,
) -> Result<Drawings, String> {
    design::at_revision(root, rev, &folders.cache, |work| {
        draw(work, folders, selection, store)
    })
}

/// Draws the selection at <work>: the screen boards from the picture folder
/// and, for those it lacks, the same drawing a shots run makes; every other
/// board and the previews in the browser (`draw_in_browser`).
fn draw(
    work: &Path,
    folders: &boards::Folders,
    selection: Selection<'_>,
    store: &Store,
) -> Result<Drawings, String> {
    let frames = design::frames(work).map_err(|e| e.to_string())?;
    let wanted: Vec<&String> = match selection {
        Selection::All => frames.keys().collect(),
        Selection::Named { boards, .. } => boards.iter().collect(),
    };
    let (screens, others): (Vec<&String>, Vec<&String>) = wanted.into_iter().partition(|name| {
        frames.contains_key(*name) && matches!(Name::parse(name), Ok(Name::Screen(_)))
    });
    let mut drawn = Drawings::new();
    if !screens.is_empty() {
        drawn.extend(draw_screens(work, folders, &screens, store)?);
    }
    let components: Vec<String> = match selection {
        Selection::All => Previews::of(work)?
            .components
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
        Selection::Named { components, .. } => components.to_vec(),
    };
    let others: Vec<String> = others.into_iter().cloned().collect();
    if others.is_empty() && components.is_empty() {
        return Ok(drawn);
    }
    let in_browser = Selection::Named {
        boards: &others,
        components: &components,
    };
    drawn.extend(draw_in_browser(work, folders, in_browser)?);
    Ok(drawn)
}

/// The drawings of the screen boards <names> of the repo at <work>, each
/// taken from the picture folder by its board hash when it keeps one.
fn draw_screens(
    work: &Path,
    folders: &boards::Folders,
    names: &[&String],
    store: &Store,
) -> Result<Drawings, String> {
    let previews = Previews::of(work)?;
    let settings = Settings::load(work).map_err(|e| e.to_string())?;
    let runtime = runtime::check(work).map_err(|e| e.to_string())?;
    let design = DesignSystem::of(work, folders).map_err(|e| e.to_string())?;
    let tree = Tree {
        root: work,
        design: &design,
        runtime: runtime.copy.text,
    };
    let names: Vec<Name> = names
        .iter()
        .map(|name| Name::parse(name).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let scales: render::Scales = names
        .iter()
        .map(|name| (name.clone(), DEVICE_SCALE))
        .collect();
    let hashes = render::hashes(
        work,
        &scales,
        &previews,
        &design,
        runtime.copy.sha256,
        store.capture_version,
        &settings,
    )?;
    let out = tempfile::tempdir_in(&folders.cache).map_err(|e| e.to_string())?;
    Drawer {
        tree,
        settings: &settings,
        previews: &previews,
        folders,
    }
    .boards_into(store.pictures, &hashes, &scales, out.path())?;
    let mut drawn = Drawings::new();
    for name in &names {
        let list = crate::shots::ElementList::load(&out.path().join(format!("{name}.json")))
            .map_err(|e| e.to_string())?;
        let file = out.path().join(format!("{name}.png"));
        let png =
            std::fs::read(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        drawn.insert(
            format!("board {name}"),
            Some(Picture::of(&png, &list.elements)?),
        );
    }
    Ok(drawn)
}

fn draw_in_browser(
    work: &Path,
    folders: &boards::Folders,
    selection: Selection<'_>,
) -> Result<Drawings, String> {
    let previews = Previews::of(work)?;
    render::with_design(
        work,
        folders,
        DEVICE_SCALE,
        async |server, chrome, frames| {
            let (boards, components): (Vec<_>, Vec<_>) = match selection {
                Selection::All => (
                    frames
                        .iter()
                        .map(|(name, frame)| (name, Some(frame)))
                        .collect(),
                    previews
                        .components
                        .iter()
                        .map(|(name, _)| (name, true))
                        .collect(),
                ),
                Selection::Named { boards, components } => {
                    let known_components: BTreeSet<_> =
                        previews.components.iter().map(|(name, _)| name).collect();
                    (
                        boards.iter().map(|name| (name, frames.get(name))).collect(),
                        components
                            .iter()
                            .map(|name| (name, known_components.contains(name)))
                            .collect(),
                    )
                }
            };
            let mut drawn = Drawings::new();
            for (name, frame) in boards {
                let picture = match frame {
                    None => None,
                    Some(frame) => Some(
                        chrome
                            .with_page(&format!("board {name}"), async |page| {
                                let rendered =
                                    board::render(page, name, &server.board_url(name), *frame)
                                        .await
                                        .map_err(|e| e.to_string())?;
                                let elements =
                                    render::read::elements(page, rendered.frame, Platform::Mac)
                                        .await
                                        .map_err(|e| e.to_string())?;
                                Picture::of(&rendered.png, &elements)
                            })
                            .await?,
                    ),
                };
                drawn.insert(format!("board {name}"), picture);
            }
            for (name, exists) in components {
                let picture = if exists {
                    Some(
                        chrome
                            .with_page(&format!("the preview of {name}"), async |page| {
                                board::load(
                                    page,
                                    name,
                                    &server.preview_url(name),
                                    preview::PREVIEW_FRAME,
                                    preview::PREVIEW_LOADED,
                                    None,
                                )
                                .await
                                .map_err(|e| e.to_string())?;
                                let rendered = preview::capture(page, preview::CaptureArea::Page)
                                    .await
                                    .map_err(|e| e.to_string())?;
                                let elements =
                                    render::read::elements(page, rendered.frame, Platform::Mac)
                                        .await
                                        .map_err(|e| e.to_string())?;
                                Picture::of(&rendered.png, &elements)
                            })
                            .await?,
                    )
                } else {
                    None
                };
                drawn.insert(format!("component {name}"), picture);
            }
            Ok(drawn)
        },
    )
}

#[cfg(test)]
#[path = "draws_tests.rs"]
mod tests;
