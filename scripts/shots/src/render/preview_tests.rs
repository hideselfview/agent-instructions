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

fn marks() -> Preview {
    Preview {
        row: ".row".to_string(),
        name: ".row-name".to_string(),
        cell: ".row-cell".to_string(),
    }
}

/// A preview page that draws two rows with React, marked as `marks` says.
const PREVIEW: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<style>body{margin:0;font:13px sans-serif}.row{display:flex}.row-cell{width:900px}</style></head>
<body><div id="root"></div><script>
const h = React.createElement;
const W = (name, child) => h('div', {className: 'row'},
  h('span', {className: 'row-name'}, name), h('div', {className: 'row-cell'}, child));
ReactDOM.createRoot(document.getElementById('root')).render(h('div', null,
  W('Label', h('div', null, 'Pause between sides and discs')),
  W('Nested, off', h(React.Fragment, null,
    h('div', null, 'Resume automatically'),
    h('button', {'aria-label': 'Never', 'aria-haspopup': 'menu'}, 'Never')))));
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
    let (row, missing) = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.preview_url("MacRow");
        let row = render_row(
            &page,
            "MacRow",
            "nested-off",
            &url,
            2.0,
            Platform::Mac,
            &marks(),
        )
        .await;
        let page = chrome.new_page().await.unwrap();
        let missing = render_row(
            &page,
            "MacRow",
            "missing",
            &url,
            1.0,
            Platform::Mac,
            &marks(),
        )
        .await;
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
        row.elements.iter().all(|e| e.bounds.y < row.size.height),
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
            let row = render_row(&page, "MacRow", "label", &url, 1.0, platform, &marks())
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
                        1.0,
                        Platform::Android,
                        &marks(),
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
