use super::*;
use crate::render::fixture;
use crate::shots::Role;
use sha2::{Digest, Sha256};
use std::time::Duration;

const REACT_SHA256: &str = "d949f1c3687aedadcedac85261865f29b17cd273997e7f6b2bfc53b2f9d4c4dd";
const REACT_DOM_SHA256: &str = "35f4f974f4b2bcd44da73963347f8952e341f83909e4498227d4e26b98f66f0d";

fn hex(text: &str) -> String {
    crate::hex::hex(&Sha256::digest(text.as_bytes()))
}

fn rows() -> &'static Preview {
    static ROWS: std::sync::OnceLock<Preview> = std::sync::OnceLock::new();
    ROWS.get_or_init(|| Preview {
        row: ".row".to_string(),
        name: ".row-name".to_string(),
        cell: ".row-cell".to_string(),
    })
}

fn marks(unshot: &[String]) -> Marks<'_> {
    Marks {
        rows: rows(),
        unshot,
    }
}

/// A preview page that draws two rows with React, marked as `marks` says.
const PREVIEW: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<style>body{margin:0;font:13px sans-serif}.row{display:flex}
.row:nth-child(2){margin-top:850px}.row-cell{width:900px;position:relative}</style></head>
<body><div id="root"></div><script>
const h = React.createElement;
const W = (name, child) => h('div', {className: 'row'},
  h('span', {className: 'row-name'}, name), h('div', {className: 'row-cell'}, child));
ReactDOM.createRoot(document.getElementById('root')).render(h('div', null,
  W('Label', h('div', null, 'Pause between sides and discs')),
  W('Nested, off', h(React.Fragment, null,
    h('div', null, 'Resume automatically'),
    h('button', {'aria-label': 'Never', 'aria-haspopup': 'menu'}, 'Never'),
    h('div', {style: {position: 'absolute', top: '80px'}}, 'Outside the cell')))));
</script></body></html>"#;

#[test]
fn the_design_system_loads_ahead_of_the_previews_script() {
    let html = "<!-- card -->\n<!doctype html>\n<html>\n<head><meta charset=\"utf-8\"></head>\n<body><script>row()</script></body></html>";
    let page = assemble("MacRow", html, "demo").unwrap();
    let head = page.find("<head>").unwrap();
    let order: Vec<usize> = [
        "/ds/demo/tokens.css",
        "/ds/demo/components/bundle.css",
        "/react.js",
        "/react-dom.js",
        "/ds/demo/components/bundle.js",
        "<meta",
        "row()",
    ]
    .iter()
    .map(|needle| {
        page.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing in {page}"))
    })
    .collect();
    assert!(order[0] > head);
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
}

#[test]
fn a_preview_with_no_head_is_refused() {
    assert!(matches!(
        assemble("MacRow", "<body></body>", "demo"),
        Err(PreviewError::NoHead(_))
    ));
}

#[test]
fn the_copies_of_react_are_the_pinned_files() {
    assert_eq!(hex(REACT), REACT_SHA256);
    assert_eq!(hex(REACT_DOM), REACT_DOM_SHA256);
}

#[tokio::test]
async fn a_row_reads_as_the_state_it_names() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/MacRow/preview.html",
        PREVIEW,
    );
    let (row, missing) = fixture::with_browser_at(root.path(), 2.0, async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.preview_url("MacRow");
        let row = render_row(
            &page,
            "MacRow",
            "nested-off",
            &url,
            Platform::Mac,
            marks(&[]),
        )
        .await;
        let page = chrome.new_page().await.unwrap();
        let missing = render_row(&page, "MacRow", "missing", &url, Platform::Mac, marks(&[])).await;
        (row, missing)
    })
    .await;
    let message = missing.err().expect("a refusal").to_string();
    assert!(
        message.contains("nested-off") && message.contains("'missing'"),
        "{message}"
    );
    let row = row.unwrap();
    let found: Vec<(Role, Option<&str>)> = row
        .read
        .elements
        .iter()
        .map(|e| (e.role, e.label.as_deref()))
        .collect();
    assert_eq!(
        found,
        [
            (Role::Text, Some("Resume automatically")),
            (Role::Popup, Some("Never"))
        ]
    );
    assert!(
        row.read
            .elements
            .iter()
            .all(|e| e.bounds.y < row.size.height),
        "boxes are inside the cell"
    );
    assert_eq!(&row.png[..8], b"\x89PNG\r\n\x1a\n");
    let width = u32::from_be_bytes(row.png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(row.png[20..24].try_into().unwrap());
    assert_eq!(
        (f64::from(width), f64::from(height)),
        (row.size.width * 2.0, row.size.height * 2.0)
    );
}

#[tokio::test]
async fn a_shared_row_gets_the_android_class_on_the_android_lane_only() {
    let root = fixture::tree(&[]);
    let page_html = PREVIEW.replace(
        "</style>",
        "body.bae-preview-android .row-cell{width:400px}</style>",
    );
    fixture::write(
        root.path(),
        "design/system/components/MacRow/preview.html",
        &page_html,
    );
    let (mac, android) = fixture::with_browser(root.path(), async |server, chrome| {
        let url = server.preview_url("MacRow");
        let mut widths = Vec::new();
        for platform in [Platform::Mac, Platform::Android] {
            let page = chrome.new_page().await.unwrap();
            let row = render_row(&page, "MacRow", "label", &url, platform, marks(&[]))
                .await
                .unwrap();
            widths.push(row.size.width);
        }
        (widths[0], widths[1])
    })
    .await;
    assert_eq!((mac, android), (900.0, 400.0));
}

/// A preview page whose android styles name a font that a file of the served
/// tree gives, so the page fetches the font once it carries the class.
const ANDROID_FONT_PREVIEW: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<style>@font-face{font-family:Late;src:url(/late.woff2)}
body{margin:0;font:13px sans-serif}.bae-preview-android .row-cell{font-family:Late}
.row{display:flex}.row-cell{width:300px}</style></head>
<body><div id="root"></div><script>
const h = React.createElement;
ReactDOM.createRoot(document.getElementById('root')).render(h('div', {className: 'row'},
  h('span', {className: 'row-name'}, 'Label'), h('div', {className: 'row-cell'}, 'Words')));
</script></body></html>"#;

#[tokio::test]
async fn an_android_row_whose_class_names_a_font_to_fetch_is_drawn() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/Late/preview.html",
        ANDROID_FONT_PREVIEW,
    );
    fixture::write(root.path(), "design/mocks/late.woff2", "not a font");
    let row = fixture::with_browser(root.path(), async |server, chrome| {
        chrome
            .with_page_within(
                "the preview of Late",
                Duration::from_secs(30),
                async |page| {
                    render_row(
                        page,
                        "Late",
                        "label",
                        &server.preview_url("Late"),
                        Platform::Android,
                        marks(&[]),
                    )
                    .await
                    .map_err(|e| e.to_string())
                },
            )
            .await
    })
    .await;
    row.unwrap();
}

/// A preview page of two rows, the second below the first. The first draws a
/// part of class `kb` that reaches down over the top of the second row's cell;
/// the second draws a part of the same class inside its own cell.
const KEYBOARD_PREVIEW: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<style>body{margin:0;font:13px sans-serif}.row{display:flex;height:300px}
.row-cell{width:400px;position:relative}
.kb{position:absolute;left:10px;top:200px;width:380px;height:80px;background:#ccc}</style></head>
<body><div id="root"></div><script>
const h = React.createElement;
const W = (name, child) => h('div', {className: 'row'},
  h('span', {className: 'row-name'}, name), h('div', {className: 'row-cell'}, child));
ReactDOM.createRoot(document.getElementById('root')).render(h('div', null,
  W('Plain', h('div', {className: 'kb', style: {top: '250px', height: '100px'}},
    h('div', {className: 'bae-preview-slot'}, 'slot'))),
  W('Typing', h('div', {className: 'kb'}, 'keys'))));
</script></body></html>"#;

#[tokio::test]
async fn a_row_reports_the_unshot_parts_inside_its_cell_from_the_cells_corner() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/MacRow/preview.html",
        KEYBOARD_PREVIEW,
    );
    let unshot = [".kb".to_string()];
    let (typing, plain, none) = fixture::with_browser(root.path(), async |server, chrome| {
        let url = server.preview_url("MacRow");
        let mut rows = Vec::new();
        for (state, unshot) in [
            ("typing", &unshot[..]),
            ("plain", &unshot[..]),
            ("typing", &[][..]),
        ] {
            let page = chrome.new_page().await.unwrap();
            rows.push(
                render_row(&page, "MacRow", state, &url, Platform::Mac, marks(unshot))
                    .await
                    .unwrap()
                    .read
                    .unshot,
            );
        }
        (rows[0].clone(), rows[1].clone(), rows[2].clone())
    })
    .await;
    let boxes: Vec<_> = typing
        .iter()
        .map(|b| (b.x, b.y, b.width, b.height))
        .collect();
    assert_eq!(boxes, [(10.0, 200.0, 380.0, 80.0)]);
    let boxes: Vec<_> = plain
        .iter()
        .map(|b| (b.x, b.y, b.width, b.height))
        .collect();
    assert_eq!(boxes, [(10.0, 250.0, 380.0, 100.0)]);
    assert!(none.is_empty(), "no selector, no boxes");
}

#[tokio::test]
async fn a_row_reports_only_the_preview_slots_inside_its_cell() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/MacRow/preview.html",
        KEYBOARD_PREVIEW,
    );
    let (typing, plain) = fixture::with_browser(root.path(), async |server, chrome| {
        let url = server.preview_url("MacRow");
        let mut rows = Vec::new();
        for state in ["typing", "plain"] {
            let page = chrome.new_page().await.unwrap();
            rows.push(
                render_row(&page, "MacRow", state, &url, Platform::Mac, marks(&[]))
                    .await
                    .unwrap()
                    .read
                    .slots,
            );
        }
        (rows[0].len(), rows[1].len())
    })
    .await;
    assert_eq!((typing, plain), (0, 1));
}

/// The whole page's picture holds the page's pixels, not the flat picture of
/// the browser window that a capture from the page returns.
#[tokio::test]
async fn a_page_capture_holds_the_previews_pixels() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/MacSwatch/preview.html",
        "<!doctype html><html><head><meta charset=\"utf-8\"></head>\
<body style=\"margin:0\"><div style=\"width:60px;height:40px;background:#f00\"></div></body></html>",
    );
    let rendered = fixture::with_browser_at(root.path(), 2.0, async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        board::load(
            &page,
            "MacSwatch",
            &server.preview_url("MacSwatch"),
            PREVIEW_FRAME,
            PREVIEW_LOADED,
            None,
        )
        .await
        .unwrap();
        capture(&page, CaptureArea::Page).await.unwrap()
    })
    .await;
    let image = image::load_from_memory(&rendered.png).unwrap().to_rgb8();
    assert_eq!(image.get_pixel(20, 20).0, [255, 0, 0]);
}

/// A preview page of two rows: the first is <first_height> points tall, the
/// second holds a white box on a black cell, its top at 65.5 points, 247.33 tall.
fn two_rows(first_height: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<style>body{{margin:0}}.row{{display:flex}}.row-cell{{width:100px;position:relative;background:#000}}
.first .row-cell{{height:{first_height}}}.second .row-cell{{height:330px}}
.box{{position:absolute;left:20px;top:65.5px;width:40px;height:247.33px;background:#fff}}</style></head>
<body><div id="root"></div><script>
const h = React.createElement;
const R = (cls, name, child) => h('div', {{className: 'row ' + cls}},
  h('span', {{className: 'row-name'}}, name), h('div', {{className: 'row-cell'}}, child));
ReactDOM.createRoot(document.getElementById('root')).render(h('div', null,
  R('first', 'First', null), R('second', 'Second', h('div', {{className: 'box'}}))));
</script></body></html>"#
    )
}

/// A row draws the same picture whatever the rows above it measure: the box in
/// the second row paints on the same device pixels when the first row is 376
/// points tall and when it is 376.33.
#[tokio::test]
async fn a_row_draws_the_same_picture_below_a_row_of_a_fractional_height() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/Whole/preview.html",
        &two_rows("376px"),
    );
    fixture::write(
        root.path(),
        "design/system/components/Fraction/preview.html",
        &two_rows("376.33px"),
    );
    let pictures = fixture::with_browser_at(root.path(), 2.0, async |server, chrome| {
        let mut pictures = Vec::new();
        for component in ["Whole", "Fraction"] {
            let page = chrome.new_page().await.unwrap();
            let row = render_row(
                &page,
                component,
                "second",
                &server.preview_url(component),
                Platform::Mac,
                marks(&[]),
            )
            .await
            .unwrap();
            pictures.push(row.png);
        }
        pictures
    })
    .await;
    let decoded: Vec<_> = pictures
        .iter()
        .map(|png| image::load_from_memory(png).unwrap().to_rgb8())
        .collect();
    let edges: Vec<_> = decoded
        .iter()
        .map(|image| {
            let white = |y: u32| image.get_pixel(80, y).0 == [255, 255, 255];
            let top = (0..image.height()).find(|&y| white(y)).unwrap();
            let bottom = (0..image.height()).rev().find(|&y| white(y)).unwrap();
            (image.width(), image.height(), top, bottom)
        })
        .collect();
    assert_eq!(edges[0], edges[1], "{edges:?}");
    assert_eq!(decoded[0], decoded[1]);
}

/// A preview page of three rows: the first draws a window element 100 by 60
/// with 12 pt corners, the second a sheet of 80 by 40 with 8 pt corners, the
/// third neither.
const WINDOW_PREVIEW: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<style>body{margin:0;font:13px sans-serif}.row{display:flex;height:100px}
.row-cell{width:200px;position:relative}
.bae-m-window{width:100px;height:60px;border-radius:12px;corner-shape:round}
.bae-m-face-sheet{width:80px;height:40px;border-radius:8px}</style></head>
<body><div id="root"></div><script>
const h = React.createElement;
const W = (name, child) => h('div', {className: 'row'},
  h('span', {className: 'row-name'}, name), h('div', {className: 'row-cell'}, child));
ReactDOM.createRoot(document.getElementById('root')).render(h('div', null,
  W('Window', h('div', {className: 'bae-m-window'})),
  W('Sheet', h('div', {className: 'bae-m-face-sheet'})),
  W('Plain', h('div', null, 'text'))));
</script></body></html>"#;

#[tokio::test]
async fn a_row_reports_the_window_or_sheet_its_cell_draws_from_the_cells_corner() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/MacRow/preview.html",
        WINDOW_PREVIEW,
    );
    let (window, sheet, plain) = fixture::with_browser(root.path(), async |server, chrome| {
        let url = server.preview_url("MacRow");
        let mut rows = Vec::new();
        for state in ["window", "sheet", "plain"] {
            let page = chrome.new_page().await.unwrap();
            rows.push(
                render_row(&page, "MacRow", state, &url, Platform::Mac, marks(&[]))
                    .await
                    .unwrap()
                    .read
                    .window,
            );
        }
        (rows[0], rows[1], rows[2])
    })
    .await;
    assert_eq!((window.frame.x, window.frame.y), (0.0, 0.0));
    assert_eq!((window.frame.width, window.frame.height), (100.0, 60.0));
    assert_eq!((window.radius, window.exponent), (12.0, 2.0));
    assert_eq!((sheet.frame.width, sheet.frame.height), (80.0, 40.0));
    assert_eq!((sheet.radius, sheet.exponent), (8.0, 2.0));
    assert_eq!(plain.radius, 0.0);
    assert!(plain.frame.width > 0.0);
}
