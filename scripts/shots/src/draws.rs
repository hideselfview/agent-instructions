//! Whether boards and component previews draw the same at two revisions.
//!
//! The checker draws each named board and each named component's preview
//! page at both revisions, in the pinned Chrome, and compares what each
//! draws: its picture and the element list the shots read from it (the
//! boxes, roles and words). A board's frame on the canvas is part of its
//! picture, so a board resized in `canvas.json` draws differently. A board or
//! preview that exists at one revision only draws differently too. The files'
//! bytes never decide: a rebase that merges unrelated lines into a board
//! file, `canvas.json` or a component's source changes bytes and no drawing.

use crate::design::{self, DesignSystem};
use crate::hex::hex;
use crate::names::{Platform, Previews};
use crate::render::{self, board, preview, runtime, serve};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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

/// The element that says a preview page has loaded.
const PREVIEW_LOADED: &str = "body";

/// The names of the boards and component previews of the repo at <root>
/// that draw the same at <at> and at <against>: the lines `alike <what>` and
/// `differs <what>`, one for each name asked.
pub(crate) fn lines(
    root: &Path,
    at: &str,
    against: &str,
    boards: &[String],
    components: &[String],
    cache: &Path,
) -> Result<Vec<String>, String> {
    let before = read(root, at, cache, boards, components)?;
    let after = read(root, against, cache, boards, components)?;
    Ok(before
        .iter()
        .map(|(what, picture)| {
            let alike = picture.is_some() && after.get(what) == Some(picture);
            format!("{} {what}", if alike { "alike" } else { "differs" })
        })
        .collect())
}

/// How the repo at <root> draws <boards> and <components> at revision <rev>.
fn read(
    root: &Path,
    rev: &str,
    cache: &Path,
    boards: &[String],
    components: &[String],
) -> Result<Drawings, String> {
    design::at_revision(root, rev, cache, |work| {
        draw(work, cache, boards, components)
    })
}

fn draw(
    work: &Path,
    cache: &Path,
    boards: &[String],
    components: &[String],
) -> Result<Drawings, String> {
    let runtime = runtime::check(work).map_err(|e| e.to_string())?;
    let design = DesignSystem::of(work, cache).map_err(|e| e.to_string())?;
    let frames = design::frames(work).map_err(|e| e.to_string())?;
    let previews = Previews::of(work)?;
    let tree = serve::Tree {
        root: work,
        design: &design,
        runtime: runtime.copy.text,
    };
    render::with_browser(tree, cache, async |server, chrome| {
        let mut drawn = Drawings::new();
        for name in boards {
            let frame = frames
                .get(name)
                .filter(|_| work.join(boards::board_path(name)).is_file());
            let picture = match frame {
                None => None,
                Some(frame) => Some(
                    chrome
                        .with_page(async |page| {
                            let rendered =
                                board::render(page, name, &server.board_url(name), *frame, 1.0)
                                    .await
                                    .map_err(|e| e.to_string())?;
                            let elements = render::read::elements(page, Platform::Mac)
                                .await
                                .map_err(|e| e.to_string())?;
                            Picture::of(&rendered.png, &elements)
                        })
                        .await
                        .map_err(|e| format!("board {name}: {e}"))?,
                ),
            };
            drawn.insert(format!("board {name}"), picture);
        }
        for name in components {
            let picture = if previews.components.iter().any(|(known, _)| known == name) {
                Some(
                    chrome
                        .with_page(async |page| {
                            board::load(
                                page,
                                name,
                                &server.preview_url(name),
                                preview::PREVIEW_FRAME,
                                1.0,
                                PREVIEW_LOADED,
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                            let png = page
                                .screenshot(
                                    chromiumoxide::page::ScreenshotParams::builder()
                                        .format(chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat::Png)
                                        .capture_beyond_viewport(true)
                                        .full_page(true)
                                        .build(),
                                )
                                .await
                                .map_err(|e| e.to_string())?;
                            let elements = render::read::elements(page, Platform::Mac)
                                .await
                                .map_err(|e| e.to_string())?;
                            Picture::of(&png, &elements)
                        })
                        .await
                        .map_err(|e| format!("the preview of {name}: {e}"))?,
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
