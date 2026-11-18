use super::*;
use crate::render::fixture;

fn tree() -> tempfile::TempDir {
    fixture::tree(&[
        (
            "fixture.wrapped",
            "<x-dc><helmet><style>body{margin:0;background:#123}.row{display:flex}</style></helmet>\
<div class=\"row\"><x-import component-from-global-scope=\"Demo.Box\"></x-import></div></x-dc>",
        ),
        (
            "fixture.broken",
            "<x-dc><div>broken</div></x-dc>\
<script type=\"text/x-dc\" data-dc-script>class Component extends DCLogic { renderVals( { }</script>",
        ),
    ])
}

#[tokio::test]
async fn an_x_import_renders_inside_its_wrapper() {
    let root = tree();
    let (rendered, display, inner) = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let rendered = render(
            &page,
            "fixture.wrapped",
            &server.board_url("fixture.wrapped"),
            Frame { w: 300, h: 200 },
            2.0,
        )
        .await;
        let display: String = page
            .evaluate("getComputedStyle(document.querySelector('.row > .sc-host-x')).display")
            .await
            .unwrap()
            .into_value()
            .unwrap();
        let inner: String = page
            .evaluate("document.querySelector('.sc-host-x #inner').textContent")
            .await
            .unwrap()
            .into_value()
            .unwrap();
        (rendered, display, inner)
    })
    .await;
    let rendered = rendered.unwrap();
    assert_eq!(display, "contents");
    assert_eq!(inner, "hello");
    assert_eq!(&rendered.png[..8], b"\x89PNG\r\n\x1a\n");
    let image_width = u32::from_be_bytes(rendered.png[16..20].try_into().unwrap());
    assert_eq!(image_width, 600);
}

#[tokio::test]
async fn a_board_renders_to_the_same_picture_twice() {
    let root = tree();
    let pictures = fixture::with_browser(root.path(), async |server, chrome| {
        let mut pictures = Vec::new();
        for _ in 0..2 {
            let page = chrome.new_page().await.unwrap();
            let url = server.board_url("fixture.wrapped");
            pictures.push(
                render(
                    &page,
                    "fixture.wrapped",
                    &url,
                    Frame { w: 300, h: 200 },
                    1.0,
                )
                .await,
            );
        }
        pictures
    })
    .await;
    let pictures: Vec<Vec<u8>> = pictures.into_iter().map(|p| p.unwrap().png).collect();
    assert_eq!(pictures[0], pictures[1]);
}

#[tokio::test]
async fn a_runtime_error_fails_the_board() {
    let root = tree();
    let result = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        render(
            &page,
            "fixture.broken",
            &server.board_url("fixture.broken"),
            Frame { w: 300, h: 200 },
            1.0,
        )
        .await
    })
    .await;
    match result {
        Err(BoardError::Runtime { board, errors }) => {
            assert_eq!(board, "fixture.broken");
            assert!(
                errors.iter().any(|e| e.contains("[dc-runtime]")),
                "{errors:?}"
            );
        }
        other => panic!(
            "expected a runtime error, got {:?}",
            other.map(|r| r.png.len())
        ),
    }
}

#[tokio::test]
async fn a_page_the_runtime_never_mounts_is_not_drawn() {
    let root = tree();
    fixture::write(
        root.path(),
        "design/mocks/fixture.plain.dc.html",
        "<!doctype html><html><body><p>no runtime</p></body></html>",
    );
    let result = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        render(
            &page,
            "fixture.plain",
            &server.board_url("fixture.plain"),
            Frame { w: 300, h: 200 },
            1.0,
        )
        .await
    })
    .await;
    match result {
        Err(BoardError::NotReady { board, state }) => {
            assert_eq!(board, "fixture.plain");
            assert_eq!(state, "nothing matches #dc-root");
        }
        other => panic!("expected not drawn, got {:?}", other.map(|r| r.png.len())),
    }
}
