use super::*;
use crate::design::Frame;
use crate::render::board::{BOARD_MOUNTED, load};
use crate::render::fixture;
use serde_json::json;

/// A board that draws `Demo.Box` with props of every kind, and `Demo.Card`,
/// which draws a `Demo.Box` of its own.
const BOARD: &str = r#"<x-dc>
<x-import component-from-global-scope="Demo.Box" kind="primary" off="{{ true }}" quiet="{{ false }}" label="hello" width="{{ 40 }}" on-click="{{ go }}"><span>inside</span></x-import>
<x-import component-from-global-scope="Demo.Card"></x-import>
</x-dc>
<script type="text/x-dc" data-dc-script>
class Component extends DCLogic { renderVals() { return { go: () => {} }; } }
</script>"#;

/// A preview page with two rows, the second drawing a `Demo.Box`, and a
/// `Demo.Box` outside every row.
const PREVIEW: &str = r#"<!doctype html><html><head><meta charset="utf-8"></head>
<body><div id="root"></div><script>
const h = React.createElement;
const N = (name, child) => h('div', {className: 'row'},
  h('span', {className: 'row-name'}, name), h('div', {className: 'row-cell'}, child));
ReactDOM.createRoot(document.getElementById('root')).render(h('div', null,
  h(Demo.Box, {kind: 'heading'}),
  N('Plain', h('p', null, 'no component')),
  N('Off', h(Demo.Box, {off: true}))));
</script></body></html>"#;

fn props(pairs: serde_json::Value) -> BTreeMap<String, serde_json::Value> {
    serde_json::from_value(pairs).unwrap()
}

#[tokio::test]
async fn a_board_names_each_component_it_draws_with_the_props_that_can_be_state() {
    let root = fixture::tree(&[("fixture.instances", BOARD)]);
    let found = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.instances");
        load(
            &page,
            "fixture.instances",
            &url,
            Frame { w: 300, h: 200 },
            1.0,
            BOARD_MOUNTED,
        )
        .await
        .unwrap();
        instances(&page, None).await
    })
    .await
    .unwrap();
    let named: Vec<&str> = found.iter().map(|i| i.component.as_str()).collect();
    assert_eq!(named, ["Box", "Card", "Box"]);
    assert_eq!(
        found[0].props,
        props(
            json!({"kind": "primary", "off": true, "quiet": false, "label": "hello", "width": 40})
        ),
        "children and the callback are no state"
    );
    assert_eq!(found[0].words, ["label"], "Box draws the words hello");
    assert!(found.iter().all(|i| !i.in_row));
}

#[tokio::test]
async fn a_preview_says_which_instances_draw_in_its_rows() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/Box/preview.html",
        PREVIEW,
    );
    let found = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.preview_url("Box");
        load(
            &page,
            "Box",
            &url,
            Frame { w: 300, h: 200 },
            1.0,
            "#root > *",
        )
        .await
        .unwrap();
        instances(&page, Some(".row")).await
    })
    .await
    .unwrap();
    let placed: Vec<(&BTreeMap<String, serde_json::Value>, bool)> =
        found.iter().map(|i| (&i.props, i.in_row)).collect();
    assert_eq!(
        placed,
        [
            (&props(json!({"kind": "heading"})), false),
            (&props(json!({"off": true})), true)
        ]
    );
}

#[tokio::test]
async fn every_instance_of_a_preview_with_no_rows_draws_its_one_state() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/Box/preview.html",
        &PREVIEW
            .replace("N('Plain', h('p', null, 'no component')),", "")
            .replace(
                "N('Off', h(Demo.Box, {off: true}))",
                "h(Demo.Box, {off: true})",
            ),
    );
    let found = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.preview_url("Box");
        load(
            &page,
            "Box",
            &url,
            Frame { w: 300, h: 200 },
            1.0,
            "#root > *",
        )
        .await
        .unwrap();
        instances(&page, Some(".row")).await
    })
    .await
    .unwrap();
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|i| i.in_row));
}
