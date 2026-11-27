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
//! Each board and each component's preview page takes its drawing from the
//! picture folder (`--pictures`) by its hash (design.rs) when the folder keeps
//! one, and the command draws only what the folder lacks and keeps it there,
//! as a shots run does for the screen boards (verify.rs). The folder holds
//! the drawings of the other revision and of every run, so a comparison of a
//! branch with its base draws only the boards and previews whose files, or
//! whose design system, the branch changed. A shared board or a preview it
//! draws also keeps the components the page draws (`states` reads them).

use crate::Checker;
use crate::cli::DrawsArgs;
use crate::design;
use crate::names::Name;
use crate::pictures::Pictures;
use crate::render;
use crate::render::Drawer;
use crate::render::pages::{self, Context, Page, SCALE, Store};
use crate::shots::ElementList;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The most a channel of a pixel may differ between two drawings of the same
/// page. Chrome's rasterizer still puts a few pixels of a gradient or an edge
/// one level off from one run to the next while the machine is busy, though
/// `chrome::ARGS` leaves it nothing else to vary; two levels of 255 is far
/// below what anyone sees as a change.
const NOISE_LEVELS: u8 = 2;

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
        boards_version: &checker.versions.boards,
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

/// Draws the selection at <work>: each screen board from the picture folder
/// and, for those it lacks, the drawing a shots run makes; every other board
/// and each component's preview page from the picture folder and, for those
/// it lacks, drawn in the browser (`draw_pages`).
fn draw(
    work: &Path,
    folders: &boards::Folders,
    selection: Selection<'_>,
    store: &Store,
) -> Result<Drawings, String> {
    let context = Context::of(work, folders)?;
    let wanted: Vec<&String> = match selection {
        Selection::All => context.frames.keys().collect(),
        Selection::Named { boards, .. } => boards.iter().collect(),
    };
    let components: Vec<&String> = match selection {
        Selection::All => context
            .previews
            .components
            .iter()
            .map(|(name, _)| name)
            .collect(),
        Selection::Named { components, .. } => components.iter().collect(),
    };
    let (screens, others): (Vec<&String>, Vec<&String>) = wanted.into_iter().partition(|name| {
        context.frames.contains_key(*name)
            && matches!(
                Name::parse(name, &context.settings.apps),
                Ok(Name::Screen(_))
            )
    });
    let mut drawn = Drawings::new();
    if !screens.is_empty() {
        drawn.extend(draw_screens(work, folders, &context, &screens, store)?);
    }
    let boards = others.into_iter().map(|name| {
        let page = context
            .frames
            .get(name)
            .map(|frame| Page::Board(name, *frame));
        (format!("board {name}"), page)
    });
    let previews = components.into_iter().map(|name| {
        let known = context
            .previews
            .components
            .iter()
            .any(|(known, _)| known == name);
        (
            format!("component {name}"),
            known.then_some(Page::Preview(name)),
        )
    });
    let pages: Vec<_> = boards.chain(previews).collect();
    if !pages.is_empty() {
        drawn.extend(draw_pages(work, folders, &context, pages, store)?);
    }
    Ok(drawn)
}

/// The drawings of the screen boards <names> of the repo at <work>, each
/// taken from the picture folder by its board hash when it keeps one.
fn draw_screens(
    work: &Path,
    folders: &boards::Folders,
    context: &Context,
    names: &[&String],
    store: &Store,
) -> Result<Drawings, String> {
    let names: Vec<Name> = names
        .iter()
        .map(|name| Name::parse(name, &context.settings.apps).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let scales: render::Scales = names.iter().map(|name| (name.clone(), SCALE)).collect();
    let hashes = render::hashes(
        work,
        &scales,
        &context.previews,
        &context.design,
        context.runtime.sha256,
        store.boards_version,
        &context.settings,
    )?;
    let out = tempfile::tempdir_in(&folders.cache).map_err(|e| e.to_string())?;
    Drawer {
        tree: context.tree(work),
        settings: &context.settings,
        previews: &context.previews,
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

/// The drawings of <pages>, each named by the line's subject (`board <name>`
/// or `component <name>`; none for a page the revision lacks): taken from the
/// picture folder by its hash when it keeps one, and otherwise drawn in the
/// browser, with the components it draws, and kept there.
fn draw_pages(
    work: &Path,
    folders: &boards::Folders,
    context: &Context,
    pages: Vec<(String, Option<Page<'_>>)>,
    store: &Store,
) -> Result<Drawings, String> {
    let out = tempfile::tempdir_in(&folders.cache).map_err(|e| e.to_string())?;
    let mut drawn = Drawings::new();
    let mut lacking = Vec::new();
    for (what, page) in pages {
        let Some(page) = page else {
            drawn.insert(what, None);
            continue;
        };
        let hash = page.hash(work, context, store.boards_version)?;
        let kept = match store.pictures {
            Some(pictures) => pictures.take_board(&hash, &hash, out.path())?,
            None => false,
        };
        if kept {
            drawn.insert(what, Some(read_kept(out.path(), &hash)?));
        } else {
            lacking.push((what, page, hash));
        }
    }
    crate::log::note(format_args!(
        "drawing {} pages the picture folder lacks",
        lacking.len()
    ));
    if lacking.is_empty() {
        return Ok(drawn);
    }
    let pages_drawn = render::with_browser(
        context.tree(work),
        folders,
        SCALE,
        async |server, chrome| {
            let mut all = Vec::new();
            for (_, page, _) in &lacking {
                all.push(pages::draw(*page, context, server, chrome).await?);
            }
            Ok(all)
        },
    )?;
    for ((what, _, hash), page) in lacking.into_iter().zip(pages_drawn) {
        let picture = Picture::of(&page.png, &page.elements)?;
        if let Some(pictures) = store.pictures {
            keep(pictures, &hash, &what, &page, out.path())?;
        }
        drawn.insert(what, Some(picture));
    }
    Ok(drawn)
}

/// The drawing the picture folder kept for <hash> and `take_board` copied to
/// <out>.
fn read_kept(out: &Path, hash: &str) -> Result<Picture, String> {
    let png_file = out.join(format!("{hash}.png"));
    let png =
        std::fs::read(&png_file).map_err(|e| format!("cannot read {}: {e}", png_file.display()))?;
    let list = ElementList::load(&out.join(format!("{hash}.json"))).map_err(|e| e.to_string())?;
    Picture::of(&png, &list.elements)
}

/// Keeps the page <page>, named <label> and drawn for <hash>, in <pictures>,
/// writing its files in <out> first.
fn keep(
    pictures: &Pictures,
    hash: &str,
    label: &str,
    page: &pages::Drawn,
    out: &Path,
) -> Result<(), String> {
    let png_file = out.join(format!("{hash}.png"));
    std::fs::write(&png_file, &page.png)
        .map_err(|e| format!("cannot write {}: {e}", png_file.display()))?;
    let list = serde_json::to_vec_pretty(&page.list(label)).map_err(|e| e.to_string())?;
    let list_file = out.join(format!("{hash}.json"));
    std::fs::write(&list_file, list)
        .map_err(|e| format!("cannot write {}: {e}", list_file.display()))?;
    pictures.keep_board(hash, hash, out)?;
    let instances = serde_json::to_string(&page.instances).map_err(|e| e.to_string())?;
    pictures.keep_instances(hash, &instances)
}

#[cfg(test)]
#[path = "draws_tests.rs"]
mod tests;
