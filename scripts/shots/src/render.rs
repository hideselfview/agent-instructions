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

use crate::design;
use crate::names::{self, Name};
use crate::settings::Settings;
use crate::shots::{ElementList, Size};
use serve::Tree;
use std::path::Path;

/// Pixels per point the boards are drawn at.
const BOARD_SCALE: f64 = 1.0;

/// Renders the board of each of `names` from `tree` into `out` as
/// `<name>.png` and `<name>.json`, in the Chrome kept in the program's cache
/// folder <cache>; starts no browser when there is nothing to render.
pub(crate) fn boards(
    tree: Tree<'_>,
    settings: &Settings,
    previews: &names::Previews,
    cache: &Path,
    names: &[Name],
    out: &Path,
) -> Result<(), String> {
    if names.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(out).map_err(|e| format!("cannot prepare {}: {e}", out.display()))?;
    with_browser(tree, cache, async |server, chrome| {
        draw_with(tree.root, settings, previews, names, out, server, chrome).await
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

/// One board drawn: its picture, size and elements.
struct Drawn {
    png: Vec<u8>,
    size: Size,
    elements: Vec<crate::shots::Element>,
    unshot: Vec<crate::shots::Bounds>,
}

async fn draw_with(
    root: &Path,
    settings: &Settings,
    previews: &names::Previews,
    names: &[Name],
    out: &Path,
    server: &serve::Server,
    chrome: &chrome::Chrome,
) -> Result<(), String> {
    for name in names {
        let board = name.to_string();
        let drawn = chrome
            .with_page(async |page| match name {
                Name::Screen(_) => {
                    draw_screen(root, settings, &board, page, server, name.platform()).await
                }
                Name::Component(component) => {
                    draw_component(settings, previews, component, page, server, name.platform())
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
            scale: BOARD_SCALE,
            elements: drawn.elements,
            unshot: drawn.unshot,
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
) -> Result<Drawn, String> {
    let frame = design::frame(root, board).map_err(|e| e.to_string())?;
    let rendered = board::render(page, board, &server.board_url(board), frame, BOARD_SCALE)
        .await
        .map_err(|e| e.to_string())?;
    let elements = read::elements(page, platform)
        .await
        .map_err(|e| e.to_string())?;
    let unshot = read::unshot(page, &settings.unshot)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Drawn {
        png: rendered.png,
        size: Size {
            width: f64::from(frame.w),
            height: f64::from(frame.h),
        },
        elements,
        unshot,
    })
}

async fn draw_component(
    settings: &Settings,
    previews: &names::Previews,
    name: &names::ComponentName,
    page: &chromiumoxide::Page,
    server: &serve::Server,
    platform: names::Platform,
) -> Result<Drawn, String> {
    let component = name.component(previews);
    let marks = settings.preview(&component).map_err(|e| e.to_string())?;
    let row = preview::render_row(
        page,
        &component,
        name.state(),
        &server.preview_url(&component),
        BOARD_SCALE,
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
    })
}
