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

use crate::design::Frame;
use crate::names::{Platform, state_id};
use crate::render::board::{self, BoardError};
use crate::render::read;
use crate::settings::Preview;
use crate::shots::Size;
use crate::shots::{Bounds, Element};
use chromiumoxide::Page;
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

/// The class the page's `<body>` carries on the android lane, which the design
/// system's preview styles key Android drawings on.
const ANDROID_CLASS: &str = "bae-preview-android";

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
#[derive(Clone, Copy)]
pub(crate) struct Marks<'a> {
    pub(crate) rows: &'a Preview,
    pub(crate) unshot: &'a [String],
}

/// A state's picture and element list.
pub(crate) struct Row {
    pub(crate) png: Vec<u8>,
    /// The cell's size in points.
    pub(crate) size: Size,
    pub(crate) elements: Vec<Element>,
    /// The boxes of the preview slots in the cell, measured from its
    /// top-left like the elements.
    pub(crate) slots: Vec<Bounds>,
    /// The boxes of the unshot parts inside the cell, measured from its
    /// top-left like the elements.
    pub(crate) unshot: Vec<Bounds>,
}

pub(crate) enum CaptureArea {
    Page,
    Cell(Bounds),
}

/// Captures the whole preview page or one cell without resizing the viewport.
pub(crate) async fn capture(page: &Page, area: CaptureArea) -> Result<board::Rendered, BoardError> {
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

/// Loads the preview at `url` and draws the row of `state`, at `scale` pixels
/// per point: the picture of its cell and the elements inside it, measured
/// from the cell's top-left, read as `platform`'s boards are. `marks` are the
/// selectors of the page's rows and unshot parts. The android lane's page body gets the class
/// `bae-preview-android` from its creation, so the fonts that class names load
/// while the page loads, and a row both lanes shoot draws on each platform's
/// own surface.
pub(crate) async fn render_row(
    page: &Page,
    component: &str,
    state: &str,
    url: &str,
    scale: f64,
    platform: Platform,
    marks: Marks<'_>,
) -> Result<Row, PreviewError> {
    let Marks {
        rows: marks,
        unshot,
    } = marks;
    board::load(
        page,
        component,
        url,
        PREVIEW_FRAME,
        scale,
        PREVIEW_MOUNTED,
        (platform == Platform::Android).then_some(ANDROID_CLASS),
    )
    .await?;
    let names: Vec<String> = page
        .evaluate(row_names(marks))
        .await
        .map_err(board::protocol)?
        .into_value()
        .map_err(board::protocol)?;
    let cell = if names.is_empty() && state == "default" {
        "document.body".to_string()
    } else {
        let index = names
            .iter()
            .position(|name| state_id(name) == state)
            .ok_or_else(|| PreviewError::NoRow {
                component: component.to_string(),
                state: state.to_string(),
                rows: names.iter().map(|n| state_id(n)).collect(),
            })?;
        format!(
            "document.querySelectorAll({})[{index}].querySelector({})",
            js(&marks.row),
            js(&marks.cell)
        )
    };
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
        .execute(
            EvaluateParams::builder()
                .expression(cell)
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
        .execute(DescribeNodeParams::builder().object_id(object).build())
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
    let slots = read::slots(page, (rendered.frame.x, rendered.frame.y)).await?;
    // The page holds every row, so the boxes of the other rows' unshot parts
    // lie outside the cell.
    let unshot = read::unshot(page, unshot, (rendered.frame.x, rendered.frame.y))
        .await?
        .into_iter()
        .filter(|b| {
            b.x < rendered.frame.width
                && b.y < rendered.frame.height
                && b.x + b.width > 0.0
                && b.y + b.height > 0.0
        })
        .collect();
    Ok(Row {
        png: rendered.png,
        size: Size {
            width: rendered.frame.width,
            height: rendered.frame.height,
        },
        elements,
        slots,
        unshot,
    })
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
