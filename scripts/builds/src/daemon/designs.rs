//! The dashboard's Designs page of a project (`/designs/<project>`): every
//! board of the repo that holds the project's design files, as it stands on
//! origin/main, in a tab per page of the repo's canvas index, and a Design
//! system tab with the system's colors, text styles and components. The page
//! reads origin/main as one commit and names it, and each board and each
//! component is a frame that loads its own page drawn from that commit, so
//! the page and its frames show the same commit when origin/main moves. A
//! board is drawn the way the review page's frames draw it
//! (`design_render::render_board`, in the dark), and a component's preview the
//! way the Design System page draws its card
//! (`design_render::render_preview`).

use std::path::Path;

use boards::preview::cards;
use boards::tokens::specimen;
use boards::{BOARD_SUFFIX, CANVAS, Source, TOKENS_JSON, canvas_size};
use serde_json::{Value, json};

use crate::design_canvas::page_of;
use crate::design_render::{FRAMES_SCRIPT, NOW, fill, render_board, render_preview};
use crate::fail;
use crate::fail::{Fail, Result};
use crate::lists::Theme;

const DESIGNS_PAGE: &str = include_str!("designs.html");
/// The dashboard's colors and fonts, which its page and the Designs page
/// share.
pub const PALETTE: &str = include_str!("palette.css");

/// Is <text> a full commit id, as the Designs page names the commit its
/// frames draw?
pub fn is_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The Designs page of project <project>, whose design files are in the
/// repo that <repo> is a checkout of, drawn from the commit origin/main
/// names there. A commit's design system is generated into <cache>. A board
/// with no size, and a part of the design system that can't be read, shows
/// as that problem in its place; a canvas index that can't be read fails the
/// page.
pub fn page(project: &str, repo: &Path, cache: &Path) -> Result<String> {
    let commit = Source::Commit { repo, rev: NOW }
        .check()?
        .expect("a commit source names a commit");
    let from = Source::Commit { repo, rev: &commit };
    let index: Value = serde_json::from_str(&from.read(CANVAS, "the canvas index")?)
        .map_err(|error| Fail::Message(format!("{}: {error}", from.label(CANVAS))))?;
    let Some(page_list) = index["pages"].as_array() else {
        fail!("{} lists no pages", from.label(CANVAS));
    };
    let Some(placed) = index["boards"].as_object() else {
        fail!(
            "{}: `boards` is not an object of board files",
            from.label(CANVAS)
        );
    };
    let mut pages = Vec::new();
    for (n, page) in page_list.iter().enumerate() {
        let (Some(id), Some(name)) = (page["id"].as_str(), page["name"].as_str()) else {
            fail!(
                "{}: page {} of `pages` ({page}) has no id or no name",
                from.label(CANVAS),
                n + 1
            );
        };
        pages.push((id, name, Vec::new()));
    }
    let Some(first) = pages.first().map(|(id, _, _)| *id) else {
        fail!("{} lists no pages", from.label(CANVAS));
    };
    let mut count = 0;
    for (file, entry) in placed {
        let Some(name) = file.strip_suffix(BOARD_SUFFIX) else {
            fail!(
                "{} places {file}, which is no board file",
                from.label(CANVAS)
            );
        };
        let on = page_of("board", file, entry, first)?;
        let Some((_, _, boards)) = pages.iter_mut().find(|(id, _, _)| *id == on) else {
            fail!(
                "{}: board {file} is on page {on}, which it does not list",
                from.label(CANVAS)
            );
        };
        let title = entry["title"].as_str();
        let drawn = match canvas_size(&index, file) {
            Ok((w, h)) => json!({ "name": name, "title": title, "w": w, "h": h }),
            Err(error) => json!({ "name": name, "title": title, "error": error.text() }),
        };
        // The canvas's rows, top to bottom and left to right.
        let at = (entry["y"].as_f64(), entry["x"].as_f64());
        boards.push((at, drawn));
        count += 1;
    }
    let pages: Vec<Value> = pages
        .into_iter()
        .map(|(id, name, mut boards)| {
            boards.sort_by(|(a, _), (b, _)| canvas_order(*a, *b));
            let boards: Vec<Value> = boards.into_iter().map(|(_, drawn)| drawn).collect();
            json!({ "id": id, "name": name, "boards": boards })
        })
        .collect();
    let system = match from
        .read(TOKENS_JSON, "the design system's tokens")
        .and_then(|text| specimen(&text))
    {
        Ok(specimen) => serde_json::to_value(specimen).expect("the specimen serializes"),
        Err(error) => json!({ "error": format!("{}: {error}", from.label(TOKENS_JSON)) }),
    };
    let components = match cards(&from, cache) {
        Ok(cards) => Value::Array(
            cards
                .into_iter()
                .map(|card| match card.height {
                    Ok(height) => json!({ "name": card.name, "height": height }),
                    Err(error) => json!({ "name": card.name, "error": error }),
                })
                .collect(),
        ),
        Err(error) => json!({ "error": error.text() }),
    };
    let data = json!({
        "project": project,
        "commit": commit,
        "boards": count,
        "base": format!("/designs/{project}/{commit}"),
        "pages": pages,
        "system": system,
        "components": components,
    });
    // `<` never appears in the JSON as itself, so no text can end the script.
    let data = data.to_string().replace('<', "\\u003c");
    Ok(fill(
        DESIGNS_PAGE,
        &[
            ("PALETTE", PALETTE),
            ("DATA", &data),
            ("FRAMES", FRAMES_SCRIPT),
        ],
    ))
}

/// Which of two boards at <a> and <b> (each its y and x on the canvas)
/// comes first in the canvas's rows: the higher, then the one further left.
/// A board the index gives no place comes before the placed ones.
fn canvas_order(
    a: (Option<f64>, Option<f64>),
    b: (Option<f64>, Option<f64>),
) -> std::cmp::Ordering {
    let key = |at: Option<f64>| at.unwrap_or(f64::NEG_INFINITY);
    key(a.0)
        .total_cmp(&key(b.0))
        .then(key(a.1).total_cmp(&key(b.1)))
}

/// Board <name> of commit <commit> of the repo <repo> is in, drawn in the
/// dark as the Designs page's frame loads it.
pub fn board(repo: &Path, commit: &str, name: &str, cache: &Path) -> Result<String> {
    render_board(
        Source::Commit { repo, rev: commit },
        name,
        Theme::Dark,
        cache,
    )
}

/// Component <name>'s preview at commit <commit> of the repo <repo> is in,
/// as the Designs page's frame loads it.
pub fn preview(repo: &Path, commit: &str, name: &str, cache: &Path) -> Result<String> {
    render_preview(Source::Commit { repo, rev: commit }, name, cache)
}

#[cfg(test)]
#[path = "designs_tests.rs"]
mod tests;
