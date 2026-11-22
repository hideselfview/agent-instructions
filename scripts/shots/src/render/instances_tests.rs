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
            None,
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
            None,
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
            None,
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

#[tokio::test]
async fn an_instance_says_whether_it_draws_a_node_and_whether_the_page_sets_its_props() {
    let board = r#"<x-dc>
<x-import component-from-global-scope="Demo.Nothing"></x-import>
<x-import component-from-global-scope="Demo.Word" value="reissue"></x-import>
<x-import component-from-global-scope="Demo.Card"><x-import component-from-global-scope="Demo.Box" kind="given"></x-import></x-import>
</x-dc>
<script type="text/x-dc" data-dc-script>
class Component extends DCLogic { renderVals() { return {}; } }
</script>"#;
    let root = fixture::tree(&[("fixture.draws", board)]);
    let found = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.draws");
        load(
            &page,
            "fixture.draws",
            &url,
            Frame { w: 300, h: 200 },
            1.0,
            BOARD_MOUNTED,
            None,
        )
        .await
        .unwrap();
        instances(&page, None).await
    })
    .await
    .unwrap();
    let seen: Vec<(&str, bool, bool)> = found
        .iter()
        .map(|i| (i.component.as_str(), i.draws, i.by_page))
        .collect();
    assert_eq!(
        seen,
        [
            ("Nothing", false, true),
            ("Word", true, true),
            ("Card", true, true),
            ("Box", true, false),
            ("Box", true, true),
        ],
        "Card draws the first Box with props of its own, and the page gives it the second"
    );
    assert_eq!(found[1].words, ["value"], "a bare string is words");
}

#[tokio::test]
async fn a_child_a_component_copies_with_an_added_prop_keeps_the_pages_props() {
    let root = fixture::tree(&[]);
    fixture::write(
        root.path(),
        "design/system/components/Frame/preview.html",
        r#"<!doctype html><html><head><meta charset="utf-8"></head>
<body><div id="root"></div><script>
const h = React.createElement;
ReactDOM.createRoot(document.getElementById('root')).render(
  h(Demo.Frame, null, h(Demo.Box, {kind: 'given'})));
</script></body></html>"#,
    );
    let found = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.preview_url("Frame");
        load(
            &page,
            "Frame",
            &url,
            Frame { w: 300, h: 200 },
            1.0,
            "#root > *",
            None,
        )
        .await
        .unwrap();
        instances(&page, None).await
    })
    .await
    .unwrap();
    let boxed = found.iter().find(|i| i.component == "Box").unwrap();
    assert!(boxed.by_page);
    assert_eq!(
        boxed.props,
        props(json!({"kind": "given"})),
        "place is Frame's"
    );
}
