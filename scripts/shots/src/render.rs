//! Rendering the design boards.

pub(crate) mod board;
pub(crate) mod chrome;
#[cfg(test)]
mod fixture;
pub(crate) mod instances;
pub(crate) mod preview;
pub(crate) mod read;
pub(crate) mod runtime;
pub(crate) mod serve;

use crate::Folders;
use crate::design::{self, DesignSystem};
use crate::names::{self, Name};
use crate::settings::Settings;
use crate::shots::{ElementList, Size};
use serve::Tree;
use std::path::Path;

/// The pixels per point each board is drawn at: the scale of the shot it is
/// compared with.
pub(crate) type Scales = std::collections::BTreeMap<Name, f64>;

/// The hash of the board picture of each board in `scales`: everything the
/// picture depends on (design.rs), the pinned browser, this program's version
/// and the board's scale included. Starts no browser.
pub(crate) fn hashes(
    root: &Path,
    scales: &Scales,
    previews: &names::Previews,
    design: &DesignSystem,
    runtime_sha256: &str,
    version: &str,
    settings: &Settings,
) -> Result<std::collections::BTreeMap<Name, String>, String> {
    let mut hashes = std::collections::BTreeMap::new();
    for (name, &scale) in scales {
        let shot = name.to_string();
        let common = design::Common {
            design,
            browser_version: chrome::PINNED,
            program_version: version,
            settings,
            scale,
        };
        let hash = match name {
            Name::Screen(_) => design::board_hash(root, &shot, runtime_sha256, &common),
            Name::Component(c) => {
                design::preview_hash(root, &shot, &c.component(previews), &common)
            }
        }
        .map_err(|e| format!("{shot}: {e}"))?;
        hashes.insert(name.clone(), hash);
    }
    Ok(hashes)
}

/// Renders the board of each of `names` from `tree` into `out` as
/// `<name>.png` and `<name>.json`, in the Chrome kept in the program's cache
/// folder <cache>; starts no browser when there is nothing to render.
pub(crate) fn boards(
    tree: Tree<'_>,
    settings: &Settings,
    previews: &names::Previews,
    cache: &Path,
    names: &[Name],
    scales: &Scales,
    out: &Path,
) -> Result<(), String> {
    if names.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(out).map_err(|e| format!("cannot prepare {}: {e}", out.display()))?;
    let drawn: Scales = names.iter().map(|n| (n.clone(), scales[n])).collect();
    with_browser(tree, cache, async |server, chrome| {
        draw_with(tree, settings, previews, &drawn, out, server, chrome).await
    })
}

/// Runs <work> on a runtime of its own with <tree> served and the pinned
/// Chrome, kept in the program's cache folder <cache>, started.
pub(crate) fn with_browser<T>(
    tree: Tree<'_>,
    cache: &Path,
    work: impl AsyncFnOnce(&serve::Server, &chrome::Chrome) -> Result<T, String>,
) -> Result<T, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(async {
            serve::with_server(tree, async |server| {
                chrome::with_chrome(cache, async |chrome| work(server, chrome).await)
                    .await
                    .map_err(|e| e.to_string())?
            })
            .await
            .map_err(|e| e.to_string())?
        })
}

/// Runs <work> with the design tree at <root> served and the pinned Chrome
/// started, given every board's frame from `canvas.json`: checks the canvas
/// runtime copy and builds the design system the boards load first.
pub(crate) fn with_design<T>(
    root: &Path,
    folders: Folders<'_>,
    work: impl AsyncFnOnce(
        &serve::Server,
        &chrome::Chrome,
        &std::collections::BTreeMap<String, design::Frame>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    let runtime = runtime::check(root).map_err(|e| e.to_string())?;
    let design = DesignSystem::of(root, folders).map_err(|e| e.to_string())?;
    let frames = design::frames(root).map_err(|e| e.to_string())?;
    let tree = Tree {
        root,
        design: &design,
        runtime: runtime.copy.text,
    };
    with_browser(tree, folders.cache, async |server, chrome| {
        work(server, chrome, &frames).await
    })
}

/// One board drawn: its picture, size and elements.
struct Drawn {
    png: Vec<u8>,
    size: Size,
    elements: Vec<crate::shots::Element>,
    unshot: Vec<crate::shots::Bounds>,
    slots: Vec<crate::shots::Bounds>,
}

async fn draw_with(
    tree: Tree<'_>,
    settings: &Settings,
    previews: &names::Previews,
    scales: &Scales,
    out: &Path,
    server: &serve::Server,
    chrome: &chrome::Chrome,
) -> Result<(), String> {
    for (name, &scale) in scales {
        let board = name.to_string();
        let drawn = chrome
            .with_page(&format!("board {board}"), async |page| match name {
                Name::Screen(_) => {
                    draw_screen(
                        tree.root,
                        settings,
                        &board,
                        page,
                        server,
                        name.platform(),
                        scale,
                    )
                    .await
                }
                Name::Component(component) => {
                    draw_component(
                        settings,
                        previews,
                        component,
                        page,
                        server,
                        name.platform(),
                        scale,
                    )
                    .await
                }
            })
            .await?;
        let file = out.join(format!("{board}.png"));
        std::fs::write(&file, &drawn.png)
            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        let list = ElementList {
            board: board.clone(),
            platform: name.platform(),
            size: drawn.size,
            scale,
            elements: drawn.elements,
            unshot: drawn.unshot,
            slots: drawn.slots,
        };
        let file = out.join(format!("{board}.json"));
        std::fs::write(
            &file,
            serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        crate::log::note(format_args!("rendered {board}"));
    }
    Ok(())
}

async fn draw_screen(
    root: &Path,
    settings: &Settings,
    board: &str,
    page: &chromiumoxide::Page,
    server: &serve::Server,
    platform: names::Platform,
    scale: f64,
) -> Result<Drawn, String> {
    let frame = design::frame(root, board).map_err(|e| e.to_string())?;
    let rendered = board::render(page, board, &server.board_url(board), frame, scale)
        .await
        .map_err(|e| e.to_string())?;
    let elements = read::elements(page, rendered.frame, platform)
        .await
        .map_err(|e| e.to_string())?;
    let unshot = read::unshot(page, &settings.unshot)
        .await
        .map_err(|e| e.to_string())?;
    let slots = read::slots(page, (rendered.frame.x, rendered.frame.y))
        .await
        .map_err(|e| e.to_string())?;
    Ok(Drawn {
        png: rendered.png,
        size: Size {
            width: rendered.frame.width,
            height: rendered.frame.height,
        },
        elements,
        unshot,
        slots,
    })
}

async fn draw_component(
    settings: &Settings,
    previews: &names::Previews,
    name: &names::ComponentName,
    page: &chromiumoxide::Page,
    server: &serve::Server,
    platform: names::Platform,
    scale: f64,
) -> Result<Drawn, String> {
    let component = name.component(previews);
    let marks = settings.preview(&component).map_err(|e| e.to_string())?;
    let row = preview::render_row(
        page,
        &component,
        name.state(),
        &server.preview_url(&component),
        scale,
        platform,
        marks,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(Drawn {
        png: row.png,
        size: row.size,
        elements: row.elements,
        unshot: Vec::new(),
        slots: row.slots,
    })
}
