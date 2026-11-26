//! A design-system component's state as a board: the named row of the
//! component's `preview.html`.
//!
//! A preview page is a bare script that draws its rows with `React` and the
//! design system's components, so it is loaded the way a claude.ai design
//! system's README sets out: the token sheet, the style sheet, React 18.3.1
//! (the copies in `canvas/`, which a test pins by hash) and the component
//! script, then the page. The app's `shots-preview` line gives the selectors
//! of a row, of the element in a row that holds the state's name, and of the
//! element that holds the drawing (settings.rs). A preview with no rows is
//! one state, `default`, drawn by the whole page.

use crate::backend;
use crate::design::Frame;
use crate::names::{Platform, state_id};
use crate::render::Drawn;
use crate::render::board::{self, BoardError};
use crate::render::chrome::DrawPage;
use crate::render::read;
use crate::settings::Preview;
use crate::shots::Bounds;
use crate::shots::Size;
use chromiumoxide::cdp::browser_protocol::dom::DescribeNodeParams;
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, Viewport};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::page::ScreenshotParams;

/// React 18.3.1, which a preview page draws its rows with.
pub(crate) const REACT: &str = include_str!("../../canvas/react.production.min.js");
pub(crate) const REACT_DOM: &str = include_str!("../../canvas/react-dom.production.min.js");

/// The page the previews are loaded in, in points; a row draws at its own
/// size inside it.
pub(crate) const PREVIEW_FRAME: Frame = Frame { w: 1000, h: 800 };

/// The element that says a preview page is drawn: the first row React drew
/// into the page's root.
const PREVIEW_MOUNTED: &str = "#root > *";

/// The element that says a preview page has loaded, for a reader that must
/// also load a preview that draws no React root, such as a static picture,
/// which draws no row: the reader waits for the page's body and not for a
/// drawn row.
pub(crate) const PREVIEW_LOADED: &str = "body";

#[derive(Debug, thiserror::Error)]
pub(crate) enum PreviewError {
    #[error(transparent)]
    Board(#[from] BoardError),
    #[error(transparent)]
    Read(#[from] read::ReadError),
    #[error("{component} has no row for the state '{state}'; its rows are {}", .rows.join(", "))]
    NoRow {
        component: String,
        state: String,
        rows: Vec<String>,
    },
    #[error("{0} has no <head> to load the design system in")]
    NoHead(String),
}

/// The preview page with the design system loaded ahead of its script.
/// The design system is the one under `/ds/<namespace>/`.
pub(crate) fn assemble(
    component: &str,
    html: &str,
    namespace: &str,
) -> Result<String, PreviewError> {
    let loads = format!(
        r#"<link rel="stylesheet" href="/ds/{namespace}/tokens.css"><link rel="stylesheet" href="/ds/{namespace}/components/bundle.css"><script src="/react.js"></script><script src="/react-dom.js"></script><script src="/ds/{namespace}/components/bundle.js"></script>"#
    );
    boards::preview::with_loads(html, &loads)
        .ok_or_else(|| PreviewError::NoHead(component.to_string()))
}

/// The names of the page's rows.
fn row_names(marks: &Preview) -> String {
    format!(
        "[...document.querySelectorAll({row})].map(row => (row.querySelector({name}) || {{}}).textContent || '')",
        row = js(&marks.row),
        name = js(&marks.name)
    )
}

/// <text> as a JavaScript string literal.
fn js(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}

/// The selectors a row is read with: those of the page's rows (`shots-preview`)
/// and those of the parts the app draws no counterpart for (`shots-unshot`).
pub(crate) struct Marks<'a> {
    pub(crate) rows: &'a Preview,
    pub(crate) unshot: &'a [String],
}

pub(crate) enum CaptureArea {
    Page,
    Cell(Bounds),
}

/// Captures the whole preview page or one cell without resizing the viewport.
/// The picture is the compositor's latest frame: a capture from the page
/// (`from_surface(false)`) returns a flat dark picture of the browser window
/// with none of the page in it.
pub(crate) async fn capture(
    page: &DrawPage,
    area: CaptureArea,
) -> Result<board::Rendered, BoardError> {
    let metrics = page.layout_metrics().await.map_err(board::protocol)?;
    let viewport = metrics.css_visual_viewport;
    let frame = match area {
        CaptureArea::Page => Bounds {
            x: -viewport.page_x,
            y: -viewport.page_y,
            width: metrics.css_content_size.width,
            height: metrics.css_content_size.height,
        },
        CaptureArea::Cell(frame) => frame,
    };
    let png = page
        .screenshot(
            ScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .clip(Viewport {
                    x: frame.x + viewport.page_x,
                    y: frame.y + viewport.page_y,
                    width: frame.width,
                    height: frame.height,
                    scale: 1.0,
                })
                .capture_beyond_viewport(true)
                .build(),
        )
        .await
        .map_err(board::protocol)?;
    Ok(board::Rendered { png, frame })
}

/// Moves the row <row> down to the next whole point when its cell <cell> starts
/// between two, and moves the rows below it with it; the row's own layout does
/// not change. A row that starts on a third of a point paints its edges a
/// device pixel from where the same row paints when it starts on a whole
/// point, and the app's shot of a row starts at the shot's own origin. The next
/// whole device pixel, a half point, paints differently from the whole point.
async fn start_on_whole_point(page: &DrawPage, row: &str, cell: &str) -> Result<(), BoardError> {
    page.evaluate(format!(
        "(() => {{ const row = {row}; const top = {cell}.getBoundingClientRect().y; \
         const shift = Math.ceil(top - 1e-6) - top; \
         if (shift > 0) {{ \
           const margin = parseFloat(getComputedStyle(row).marginTop); \
           if (Number.isNaN(margin)) throw new Error('the row has no top margin in points'); \
           row.style.marginTop = (margin + shift) + 'px'; }} }})()"
    ))
    .await
    .map_err(board::protocol)?;
    Ok(())
}

/// Loads the preview at `url` and draws the row of `state`, at the page's device
/// scale: the picture of its cell and the elements inside it, measured
/// from the cell's top-left, read as `platform`'s boards are. `marks` are the
/// selectors of the page's rows and unshot parts. The android lane's page body gets the class
/// `bae-preview-android` from its creation, so the fonts that class names load
/// while the page loads, and a row both lanes shoot draws on each platform's
/// own surface.
pub(crate) async fn render_row(
    page: &DrawPage,
    component: &str,
    state: &str,
    url: &str,
    platform: Platform,
    marks: Marks<'_>,
) -> Result<Drawn, PreviewError> {
    let Marks {
        rows: marks,
        unshot,
    } = marks;
    board::load(
        page,
        component,
        url,
        PREVIEW_FRAME,
        PREVIEW_MOUNTED,
        backend::of(platform).preview_body_class(),
    )
    .await?;
    let names: Vec<String> = page
        .evaluate(row_names(marks))
        .await
        .map_err(board::protocol)?
        .into_value()
        .map_err(board::protocol)?;
    let (row, cell) = if names.is_empty() && state == "default" {
        (None, "document.body".to_string())
    } else {
        let index = names
            .iter()
            .position(|name| state_id(name) == state)
            .ok_or_else(|| PreviewError::NoRow {
                component: component.to_string(),
                state: state.to_string(),
                rows: names.iter().map(|n| state_id(n)).collect(),
            })?;
        let row = format!("document.querySelectorAll({})[{index}]", js(&marks.row));
        let cell = format!("{row}.querySelector({})", js(&marks.cell));
        (Some(row), cell)
    };
    if let Some(row) = row {
        start_on_whole_point(page, &row, &cell).await?;
    }
    let rect: Bounds = page
        .evaluate(format!(
            "(() => {{ const r = {cell}.getBoundingClientRect(); \
             return {{x: r.x, y: r.y, width: r.width, height: r.height}}; }})()"
        ))
        .await
        .map_err(board::protocol)?
        .into_value()
        .map_err(board::protocol)?;
    let object = page
        .run(
            EvaluateParams::builder()
                .expression(cell.clone())
                .build()
                .map_err(board::protocol)?,
        )
        .await
        .map_err(board::protocol)?
        .result
        .result
        .object_id
        .ok_or_else(|| board::protocol("the row's cell is not an object"))?;
    let described = page
        .run(DescribeNodeParams::builder().object_id(object).build())
        .await
        .map_err(board::protocol)?;
    let rendered = capture(page, CaptureArea::Cell(rect)).await?;
    let elements = read::elements_in(
        page,
        *described.result.node.backend_node_id.inner(),
        rendered.frame,
        platform,
    )
    .await?;
    let slots = read::slots(page, &cell, (rendered.frame.x, rendered.frame.y)).await?;
    let unshot = read::unshot(page, unshot, &cell, (rendered.frame.x, rendered.frame.y)).await?;
    let window = read::window(page, &cell, rendered.frame).await?;
    Ok(Drawn {
        png: rendered.png,
        size: Size {
            width: rendered.frame.width,
            height: rendered.frame.height,
        },
        elements,
        slots,
        unshot,
        window,
    })
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
