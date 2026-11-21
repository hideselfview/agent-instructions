//! The dashboard's Designs page of a project (`/designs/<project>`): every
//! board of the repo that holds the project's design files, as it stands on
//! origin/main, in a tab per page of the repo's canvas index, and a Design
//! system tab with the system's colors, text styles and components. The page
//! reads origin/main as one commit and names it, and each board and each
//! component is a frame that loads its own page drawn from that commit, so
//! the page and its frames show the same commit when origin/main moves. A
//! page's boards stand on a canvas the person pans and zooms, each at the x,
//! y, width and height that the index gives it. A board is drawn the way the review page's frames draw it
//! (`design_render::render_board`, in the dark), and a component's preview the
//! way the Design System page draws its card
//! (`design_render::render_preview`).
//!
//! The Design system tab shows the colors in the groups of their roles that
//! the design system's `colors.json` lists, each group under its line and
//! in a frame of its own that draws its swatches and its scene
//! (`color_group.rs`). Above the groups it shows the comparison, with its own
//! switches, then one switch for each table that names one and one for the
//! theme, which redraw every group's frame.
//!
//! The Design system tab shows the components in the groups and subgroups
//! that the design system's `groups.json` lists, each under the line that
//! introduces it (`boards::preview::contents`). Each component's
//! frame draws its preview at the preview's own size, up to the width of the
//! tab (`design_render/preview_fit.js`).
//!
//! The Scratch tab lists the scratch boards of the repo's worktrees, each a
//! link to its scratch view (`scratch_view.rs`). They are files that no
//! commit holds, so the page has the tab only when it is asked for on this
//! Mac.

use std::path::Path;

use boards::colors;
use boards::preview::{Card, contents};
use boards::tokens::specimen;
use boards::{BOARD_SUFFIX, CANVAS, Source, TOKENS_JSON, canvas_place, canvas_size};
use serde_json::{Value, json};

use crate::design_canvas::{CANVASES, page_of, pages_of, parse_canvases, placed};
use crate::design_render::{FRAMES_SCRIPT, NOW, fill, preview_frame, render_board};
use crate::fail;
use crate::fail::{Fail, Result};
use crate::lists::publish::{Request, Standing, ask_and_tell};
use crate::lists::store::Store;
use crate::lists::{Stamp, Theme};
use crate::places::Places;

const DESIGNS_PAGE: &str = include_str!("designs.html");
/// The Filter field's matching and hiding, which the page's script calls.
const FILTER: &str = include_str!("designs_filter.js");
/// The dashboard's colors and fonts, which its page and the Designs page
/// share.
pub const PALETTE: &str = include_str!("palette.css");
/// The dashboard's buttons and modal form, which its page and the Designs
/// page share.
pub const CONTROLS: &str = include_str!("controls.css");
/// The buttons and other controls both pages build the same way.
pub const CONTROLS_JS: &str = include_str!("controls.js");

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
/// <standing> is the project's publishing as its store holds it, which the
/// page's Publish to claude.ai form shows, and <scratch> the scratch boards
/// that its Scratch tab lists (`Model::scratch_boards`).
pub fn page(
    project: &str,
    repo: &Path,
    cache: &Path,
    standing: Result<Standing>,
    scratch: &Value,
) -> Result<String> {
    let commit = Source::Commit { repo, rev: NOW }
        .check()?
        .expect("a commit source names a commit");
    let from = Source::Commit { repo, rev: &commit };
    let index: Value = serde_json::from_str(&from.read(CANVAS, "the canvas index")?)
        .map_err(|error| Fail::Message(format!("{}: {error}", from.label(CANVAS))))?;
    let mut pages = Vec::new();
    for page in pages_of(&index)? {
        let Some(name) = page.entry["name"].as_str() else {
            fail!("{}: page {} has no name", from.label(CANVAS), page.id);
        };
        pages.push((page.id, name, Vec::new()));
    }
    let first = pages[0].0;
    for (file, entry) in placed(&index)? {
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
        let drawn = match (canvas_place(&index, file), canvas_size(&index, file)) {
            (Ok((x, y)), Ok((w, h))) => {
                json!({ "name": name, "title": title, "x": x, "y": y, "w": w, "h": h })
            }
            (Err(error), _) | (_, Err(error)) => {
                json!({ "name": name, "title": title, "error": error.text() })
            }
        };
        boards.push(drawn);
    }
    let pages: Vec<Value> = pages
        .into_iter()
        .map(|(id, name, boards)| json!({ "id": id, "name": name, "boards": boards }))
        .collect();
    let system = match from
        .read(TOKENS_JSON, "the design system's tokens")
        .and_then(|text| specimen(&text))
    {
        Ok(specimen) => serde_json::to_value(specimen).expect("the specimen serializes"),
        Err(error) => json!({ "error": format!("{}: {error}", from.label(TOKENS_JSON)) }),
    };
    let colors = match colors::read(&from) {
        Ok(palette) => json!({
            "themes": palette.themes,
            "switches": palette.switches,
            "comparison": palette.colors.comparison.as_ref().map(|comparison| json!({
                "name": comparison.name,
                "about": comparison.about,
                "switch": comparison.switch,
            })),
            "groups": palette.colors.groups.iter().map(|group| json!({
                "name": group.name,
                "about": group.about,
                "colors": group.names().collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }),
        Err(error) => json!({ "error": error.text() }),
    };
    let components = match contents(&from, cache) {
        Ok(contents) => json!({
            "groups": contents.groups.into_iter().map(|group| json!({
                "name": group.name,
                "about": group.about,
                "cards": cards_json(group.cards),
                "subgroups": group.subgroups.into_iter().map(|sub| json!({
                    "name": sub.name,
                    "about": sub.about,
                    "cards": cards_json(sub.cards),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "problems": contents.problems,
        }),
        Err(error) => json!({ "error": error.text() }),
    };
    let publish =
        match publishing(&from).and_then(|names_artifacts| Ok((names_artifacts, standing?))) {
            Ok((false, _)) => json!({ "canvas": false }),
            Ok((true, standing)) => json!({
                "canvas": true,
                "standing": standing,
            }),
            Err(error) => json!({ "error": error.text() }),
        };
    let data = json!({
        "project": project,
        "publish": publish,
        "commit": commit,
        "base": format!("/designs/{project}/{commit}"),
        "pages": pages,
        "system": system,
        "colors": colors,
        "components": components,
        "scratch": scratch,
    });
    // `<` never appears in the JSON as itself, so no text can end the script.
    let data = data.to_string().replace('<', "\\u003c");
    Ok(fill(
        DESIGNS_PAGE,
        &[
            ("PALETTE", PALETTE),
            ("CONTROLS", CONTROLS),
            ("CONTROLS_JS", CONTROLS_JS),
            ("DATA", &data),
            ("FRAMES", FRAMES_SCRIPT),
            ("FILTER", FILTER),
        ],
    ))
}

/// <cards> as the page's data holds them: each card's name, and its
/// height or why it has none.
fn cards_json(cards: Vec<Card>) -> Value {
    cards
        .into_iter()
        .map(|card| match card.height {
            Ok(height) => json!({ "name": card.name, "height": height }),
            Err(error) => json!({ "name": card.name, "error": error }),
        })
        .collect()
}

/// Whether `CANVASES` at <from> names the artifacts that the designs publish
/// to (a canvas or more, and the design system), by title. False when <from>
/// has no `CANVASES`, so its designs publish nowhere yet; the artifacts'
/// links are the publishing session's to find on its account.
fn publishing(from: &Source) -> Result<bool> {
    if !from.has(CANVASES)? {
        return Ok(false);
    }
    parse_canvases(
        &from.read(CANVASES, "the canvases file")?,
        &from.label(CANVASES),
    )?;
    Ok(true)
}

/// The publishing of project <project> as its store holds it.
pub fn standing(places: &Places, project: &str) -> Result<Standing> {
    let store = Store::named(places, project)?;
    let db = store.open_existing()?;
    crate::lists::publish::standing(&db)
}

/// Asks the design session that listens for project <project> to publish
/// the designs at <rev> of the repo <repo> is in, for the person at the
/// dashboard at <now> (`publish::ask_and_tell`). Fails when <rev> names no
/// commit or one with no `CANVASES`, so that nothing publishes it.
pub fn ask(places: &Places, project: &str, repo: &Path, rev: &str, now: i64) -> Result<Request> {
    let commit = Source::Commit { repo, rev }
        .check()?
        .expect("a commit source names a commit");
    let from = Source::Commit { repo, rev: &commit };
    if !from.has(CANVASES)? {
        fail!("commit {commit} has no {CANVASES}, so its designs publish to no artifact");
    }
    let store = Store::named(places, project)?;
    let stamp = Stamp {
        at: now,
        by: super::approve::BY.to_string(),
    };
    ask_and_tell(&store, &commit, &stamp)
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

/// Group <n> of the colors at commit <commit> of the repo <repo> is in, as
/// the Designs page's frame loads it (`color_group::frame`).
pub fn color_group(repo: &Path, commit: &str, n: usize, cache: &Path) -> Result<String> {
    super::color_group::frame(Source::Commit { repo, rev: commit }, n, cache)
}

/// The colors' comparison at commit <commit> of the repo <repo> is in, as
/// the Designs page's frame loads it (`color_group::comparison`).
pub fn color_comparison(repo: &Path, commit: &str, cache: &Path) -> Result<String> {
    super::color_group::comparison(Source::Commit { repo, rev: commit }, cache)
}

/// Component <name>'s preview at commit <commit> of the repo <repo> is in,
/// as the Designs page's frame loads it: with `PREVIEW_FIT`, which draws it
/// at its own size and tells the page that size.
pub fn preview(repo: &Path, commit: &str, name: &str, cache: &Path) -> Result<String> {
    preview_frame(Source::Commit { repo, rev: commit }, name, cache, false)
}

#[cfg(test)]
#[path = "designs_tests.rs"]
mod tests;
