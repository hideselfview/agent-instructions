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
use crate::render::Size;
use crate::render::board::{self, BoardError};
use crate::render::read;
use crate::settings::Preview;
use crate::shots::Element;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::dom::DescribeNodeParams;
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, Viewport};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::page::ScreenshotParams;
use regex::Regex;
use serde::Deserialize;

/// React 18.3.1, which a preview page draws its rows with.
pub(crate) const REACT: &str = include_str!("../../canvas/react.production.min.js");
pub(crate) const REACT_DOM: &str = include_str!("../../canvas/react-dom.production.min.js");

/// The page the previews are loaded in, in points; a row draws at its own
/// size inside it.
const PREVIEW_FRAME: Frame = Frame { w: 1000, h: 800 };

/// The element that says a preview page is drawn: the first row React drew
/// into the page's root.
const PREVIEW_MOUNTED: &str = "#root > *";

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
    let head = Regex::new(r"(?i)<head[^>]*>").unwrap();
    let Some(found) = head.find(html) else {
        return Err(PreviewError::NoHead(component.to_string()));
    };
    let loads = format!(
        r#"<link rel="stylesheet" href="/ds/{namespace}/tokens.css"><link rel="stylesheet" href="/ds/{namespace}/components/bundle.css"><script src="/react.js"></script><script src="/react-dom.js"></script><script src="/ds/{namespace}/components/bundle.js"></script>"#
    );
    Ok(format!(
        "{}{loads}{}",
        &html[..found.end()],
        &html[found.end()..]
    ))
}

#[derive(Deserialize)]
struct Cell {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
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

/// A state's picture and element list.
pub(crate) struct Row {
    pub(crate) png: Vec<u8>,
    /// The cell's size in points.
    pub(crate) size: Size,
    pub(crate) elements: Vec<Element>,
}

/// Loads the preview at `url` and draws the row of `state`, at `scale` pixels
/// per point: the picture of its cell and the elements inside it, measured
/// from the cell's top-left, read as `platform`'s boards are. `marks` are the
/// selectors of the page's rows.
pub(crate) async fn render_row(
    page: &Page,
    component: &str,
    state: &str,
    url: &str,
    scale: f64,
    platform: Platform,
    marks: &Preview,
) -> Result<Row, PreviewError> {
    board::load(page, component, url, PREVIEW_FRAME, scale, PREVIEW_MOUNTED).await?;
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
    let rect: Cell = page
        .evaluate(format!(
            "(() => {{ const r = {cell}.getBoundingClientRect(); \
             return {{x: r.x + scrollX, y: r.y + scrollY, width: r.width, height: r.height}}; }})()"
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
    let clip = Viewport::builder()
        .x(rect.x)
        .y(rect.y)
        .width(rect.width)
        .height(rect.height)
        .scale(1.0)
        .build()
        .map_err(board::protocol)?;
    let png = page
        .screenshot(
            ScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .clip(clip)
                .capture_beyond_viewport(true)
                .build(),
        )
        .await
        .map_err(board::protocol)?;
    let elements = read::elements_in(
        page,
        *described.result.node.backend_node_id.inner(),
        (rect.x, rect.y),
        platform,
    )
    .await?;
    Ok(Row {
        png,
        size: Size {
            width: rect.width,
            height: rect.height,
        },
        elements,
    })
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
