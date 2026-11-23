//! Whether boards and component previews draw the same at two revisions.
//!
//! The checker draws the selected boards and component previews at both
//! revisions in the pinned Chrome. With no selectors, it compares every
//! drawing from either revision. It compares what each
//! draws: its picture and the element list the shots read from it (the
//! boxes, roles and words). A board's frame on the canvas is part of its
//! picture, so a board resized in `canvas.json` draws differently. A board or
//! preview that exists at one revision only draws differently, and one that
//! exists at neither draws nothing at both, so it draws alike when their merge
//! base holds it; a name that no revision holds is an error. The files'
//! bytes never decide: a rebase that merges unrelated lines into a board
//! file, `canvas.json` or a component's source changes bytes and no drawing.

use crate::design;
use crate::hex::hex;
use crate::names::{Platform, Previews};
use crate::render::{self, board, preview};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What a board or a preview draws: the hash of its picture and its element
/// list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picture {
    png: String,
    elements: String,
}

impl Picture {
    fn of(png: &[u8], elements: &[crate::shots::Element]) -> Result<Self, String> {
        Ok(Self {
            png: hex(&Sha256::digest(png)),
            elements: serde_json::to_string(elements).map_err(|e| e.to_string())?,
        })
    }
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

/// The names of the boards and component previews of the repo at <root>
/// that draw the same at <at> and at <against>: the lines `alike <what>` and
/// `differs <what>`, one for each selected drawing.
pub(crate) fn lines(
    root: &Path,
    at: &str,
    against: &str,
    boards: &[String],
    components: &[String],
    cache: &Path,
) -> Result<Vec<String>, String> {
    let selection = if boards.is_empty() && components.is_empty() {
        Selection::All
    } else {
        Selection::Named { boards, components }
    };
    let before = read(root, at, cache, selection)?;
    let after = read(root, against, cache, selection)?;
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
            let alike = before == after;
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
    cache: &Path,
    selection: Selection<'_>,
) -> Result<Drawings, String> {
    design::at_revision(root, rev, cache, |work| draw(work, cache, selection))
}

fn draw(work: &Path, cache: &Path, selection: Selection<'_>) -> Result<Drawings, String> {
    let previews = Previews::of(work)?;
    render::with_design(work, cache, async |server, chrome, frames| {
        let known_components: BTreeSet<&str> = previews
            .components
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        let (boards, components): (Vec<&str>, Vec<&str>) = match selection {
            Selection::All => (
                frames.keys().map(String::as_str).collect(),
                known_components.iter().copied().collect(),
            ),
            Selection::Named { boards, components } => (
                boards.iter().map(String::as_str).collect(),
                components.iter().map(String::as_str).collect(),
            ),
        };
        let mut drawn = Drawings::new();
        for name in boards {
            let frame = frames
                .get(name)
                .filter(|_| work.join(boards::board_path(name)).is_file());
            let picture = match frame {
                None if matches!(selection, Selection::All) => continue,
                None => None,
                Some(frame) => Some(
                    chrome
                        .with_page(&format!("board {name}"), async |page| {
                            let rendered =
                                board::render(page, name, &server.board_url(name), *frame, 1.0)
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
        for name in components {
            let picture = if known_components.contains(name) {
                Some(
                    chrome
                        .with_page(&format!("the preview of {name}"), async |page| {
                            board::load(
                                page,
                                name,
                                &server.preview_url(name),
                                preview::PREVIEW_FRAME,
                                1.0,
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
    })
}

#[cfg(test)]
#[path = "draws_tests.rs"]
mod tests;
