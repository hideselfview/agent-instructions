//! The dashboard's Designs page of a project (`/designs/<project>`): every
//! board of the repo that holds the project's design files, as it stands on
//! origin/main, in a tab per page of the repo's canvas index, and a Design
//! system tab with the system's colors, text styles and components. The page
//! reads origin/main as one commit and names it, and each board and each
//! component is a frame that loads its own page drawn from that commit, so
//! the page and its frames show the same commit when origin/main moves. A
//! page's boards stand on a canvas the person pans and zooms, each at the x,
//! y, width and height that the index gives it, and the column at the page's
//! right lists the page's row headings (`designs_headings.rs`). A board is drawn the way the review page's frames draw it
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

use std::collections::BTreeMap;
use std::path::Path;

use boards::colors;
use boards::colors::{Comparison, Switch};
use boards::preview::{Card, contents};
use boards::tokens::{Specimen, Theme as ColorTheme, specimen};
use boards::{BOARD_SUFFIX, CANVAS, Source, TOKENS_JSON, canvas_place, canvas_size};
use serde_json::Value;

use super::designs_headings::{Entry, headings};
use super::scratch_view::Scratch;
use crate::design_canvas::{CANVASES, page_of, pages_of, parse_canvases, placed};
use crate::design_render::{FRAMES_SCRIPT, NOW, fill, preview_frame, render_board};
use crate::fail;
use crate::fail::{Fail, Result};
use crate::lists::publish::{Request, Standing, ask_and_tell};
use crate::lists::store::Store;
use crate::lists::{Stamp, Theme};
use crate::places::Places;
use boards::Folders;

const DESIGNS_PAGE: &str = include_str!("designs.html");
/// The Filter field's matching and hiding, which the page's script calls.
const FILTER: &str = include_str!("designs_filter.js");
/// The contents column's entries and marks, which the page's script calls.
const CONTENTS: &str = include_str!("designs_contents.js");
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
    scratch: Option<Scratch>,
) -> Result<String> {
    Ok(fill_page(&data(project, repo, cache, standing, scratch)?))
}

/// What the Designs page of a project shows, as `fill_page` puts it in the
/// page's script, and as a staged fixture gives it
/// (`builds dashboard stage`). Each part of the design system that can't be
/// read shows as its own error.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignsData {
    pub project: String,
    pub publish: Publish,
    /// The commit every frame of the page draws.
    pub commit: String,
    /// The address the frames' pages stand under.
    pub base: String,
    pub pages: Vec<DesignsPageData>,
    pub system: System,
    pub colors: Colors,
    pub components: Components,
    pub scratch: Option<Scratch>,
}

/// A page of the repo's canvas index: its boards and the headings of its rows.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignsPageData {
    pub id: String,
    pub name: String,
    pub boards: Vec<DesignsBoard>,
    pub headings: Vec<Entry>,
}

/// A board of a page: where the canvas index puts it and how large, or why
/// it has no place or size.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(untagged, deny_unknown_fields)]
pub enum DesignsBoard {
    Failed {
        name: String,
        title: Option<String>,
        error: String,
    },
    Placed {
        name: String,
        title: Option<String>,
        x: f64,
        y: f64,
        w: u64,
        h: u64,
    },
}

impl DesignsBoard {
    pub fn name(&self) -> &str {
        match self {
            DesignsBoard::Failed { name, .. } | DesignsBoard::Placed { name, .. } => name,
        }
    }

    /// The y of the board's place; None for a board with no place.
    pub fn y(&self) -> Option<f64> {
        match self {
            DesignsBoard::Failed { .. } => None,
            DesignsBoard::Placed { y, .. } => Some(*y),
        }
    }
}

/// The design system's text styles, or why they can't be read.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum System {
    Failed { error: String },
    Read(Specimen),
}

/// The design system's colors, or why they can't be read.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Colors {
    Failed {
        error: String,
    },
    Read {
        themes: Vec<ColorTheme>,
        switches: Vec<Switch>,
        comparison: Option<Comparison>,
        groups: Vec<ColorsGroup>,
    },
}

/// A group of colors: the names of the colors its swatches show.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorsGroup {
    pub name: String,
    pub about: String,
    pub colors: Vec<String>,
}

/// The design system's components, or why they can't be listed.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Components {
    Failed {
        error: String,
    },
    Read {
        groups: Vec<ComponentsGroup>,
        problems: Vec<String>,
    },
}

/// A group of components, with the cards it holds itself and its subgroups.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentsGroup {
    pub name: String,
    pub about: String,
    pub cards: Vec<ComponentCard>,
    pub subgroups: Vec<ComponentsSubgroup>,
}

/// A subgroup of components.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentsSubgroup {
    pub name: String,
    pub about: String,
    pub cards: Vec<ComponentCard>,
}

/// A component's card: its height, or why it has none.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ComponentCard {
    Failed { name: String, error: String },
    Drawn { name: String, height: u64 },
}

/// What the Publish to claude.ai form shows: why the publishing can't be
/// read, that the repo publishes nowhere yet, or the project's publishing.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Publish {
    Failed { error: String },
    Standing { canvas: bool, standing: Standing },
    Nowhere { canvas: bool },
}

/// The data of `page`, read from the repo's origin/main.
pub fn data(
    project: &str,
    repo: &Path,
    cache: &Path,
    standing: Result<Standing>,
    scratch: Option<Scratch>,
) -> Result<DesignsData> {
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
            (Ok((x, y)), Ok((w, h))) => DesignsBoard::Placed {
                name: name.to_string(),
                title: title.map(str::to_string),
                x,
                y,
                w,
                h,
            },
            (Err(error), _) | (_, Err(error)) => DesignsBoard::Failed {
                name: name.to_string(),
                title: title.map(str::to_string),
                error: error.text(),
            },
        };
        boards.push(drawn);
    }
    let drawn: BTreeMap<&str, &[DesignsBoard]> = pages
        .iter()
        .map(|(id, _, boards)| (*id, boards.as_slice()))
        .collect();
    let mut headings = headings(&index, first, &drawn)?;
    let pages: Vec<DesignsPageData> = pages
        .into_iter()
        .map(|(id, name, boards)| {
            let rows = headings.remove(id).unwrap_or_default();
            DesignsPageData {
                id: id.to_string(),
                name: name.to_string(),
                boards,
                headings: rows,
            }
        })
        .collect();
    if let Some(page) = headings.keys().next() {
        fail!(
            "{}: a note is on page {page}, which it does not list",
            from.label(CANVAS)
        );
    }
    let system = match from
        .read(TOKENS_JSON, "the design system's tokens")
        .and_then(|text| specimen(&text))
    {
        Ok(specimen) => System::Read(specimen),
        Err(error) => System::Failed {
            error: format!("{}: {error}", from.label(TOKENS_JSON)),
        },
    };
    let colors = match colors::read(&from) {
        Ok(palette) => Colors::Read {
            themes: palette.themes.to_vec(),
            switches: palette.switches,
            comparison: palette.colors.comparison,
            groups: palette
                .colors
                .groups
                .iter()
                .map(|group| ColorsGroup {
                    name: group.name.clone(),
                    about: group.about.clone(),
                    colors: group.names().map(str::to_string).collect(),
                })
                .collect(),
        },
        Err(error) => Colors::Failed {
            error: error.text(),
        },
    };
    let components = match contents(&from, cache) {
        Ok(contents) => Components::Read {
            groups: contents
                .groups
                .into_iter()
                .map(|group| ComponentsGroup {
                    name: group.name,
                    about: group.about,
                    cards: cards_of(group.cards),
                    subgroups: group
                        .subgroups
                        .into_iter()
                        .map(|sub| ComponentsSubgroup {
                            name: sub.name,
                            about: sub.about,
                            cards: cards_of(sub.cards),
                        })
                        .collect(),
                })
                .collect(),
            problems: contents.problems,
        },
        Err(error) => Components::Failed {
            error: error.text(),
        },
    };
    let publish =
        match publishing(&from).and_then(|names_artifacts| Ok((names_artifacts, standing?))) {
            Ok((false, _)) => Publish::Nowhere { canvas: false },
            Ok((true, standing)) => Publish::Standing {
                canvas: true,
                standing,
            },
            Err(error) => Publish::Failed {
                error: error.text(),
            },
        };
    Ok(DesignsData {
        project: project.to_string(),
        publish,
        base: format!("/designs/{project}/{commit}"),
        commit,
        pages,
        system,
        colors,
        components,
        scratch,
    })
}

/// The Designs page that shows <data>.
pub fn fill_page(data: &DesignsData) -> String {
    // `<` never appears in the JSON as itself, so no text can end the script.
    let data = serde_json::to_string(data)
        .expect("the Designs data serializes")
        .replace('<', "\\u003c");
    fill(
        DESIGNS_PAGE,
        &[
            ("PALETTE", PALETTE),
            ("CONTROLS", CONTROLS),
            ("CONTROLS_JS", CONTROLS_JS),
            ("DATA", &data),
            ("FRAMES", FRAMES_SCRIPT),
            ("FILTER", FILTER),
            ("CONTENTS", CONTENTS),
        ],
    )
}

/// <cards> as the page's data holds them: each card's name, and its
/// height or why it has none.
fn cards_of(cards: Vec<Card>) -> Vec<ComponentCard> {
    cards
        .into_iter()
        .map(|card| match card.height {
            Ok(height) => ComponentCard::Drawn {
                name: card.name,
                height,
            },
            Err(error) => ComponentCard::Failed {
                name: card.name,
                error,
            },
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
/// dashboard at <now> (`publish::ask_and_tell`), telling it through
/// <sessions>. Fails when <rev> names no commit or one with no `CANVASES`,
/// so that nothing publishes it.
pub fn ask(
    places: &Places,
    sessions: &crate::inbox::Sessions,
    project: &str,
    repo: &Path,
    rev: &str,
    now: i64,
) -> Result<Request> {
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
    ask_and_tell(&store, sessions, &commit, &stamp)
}

/// Board <name> of commit <commit> of the repo <repo> is in, drawn in the
/// dark as the Designs page's frame loads it.
pub fn board(repo: &Path, commit: &str, name: &str, dirs: &Folders) -> Result<String> {
    render_board(
        Source::Commit { repo, rev: commit },
        name,
        Theme::Dark,
        dirs,
    )
}

/// Group <n> of the colors at commit <commit> of the repo <repo> is in, as
/// the Designs page's frame loads it (`color_group::frame`).
pub fn color_group(repo: &Path, commit: &str, n: usize, dirs: &Folders) -> Result<String> {
    super::color_group::frame(Source::Commit { repo, rev: commit }, n, dirs)
}

/// The colors' comparison at commit <commit> of the repo <repo> is in, as
/// the Designs page's frame loads it (`color_group::comparison`).
pub fn color_comparison(repo: &Path, commit: &str, dirs: &Folders) -> Result<String> {
    super::color_group::comparison(Source::Commit { repo, rev: commit }, dirs)
}

/// Component <name>'s preview at commit <commit> of the repo <repo> is in,
/// as the Designs page's frame loads it: with `PREVIEW_FIT`, which draws it
/// at its own size and tells the page that size.
pub fn preview(repo: &Path, commit: &str, name: &str, dirs: &Folders) -> Result<String> {
    preview_frame(Source::Commit { repo, rev: commit }, name, dirs, false)
}

#[cfg(test)]
#[path = "designs_tests.rs"]
mod tests;
