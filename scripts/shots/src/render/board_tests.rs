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
            "fixture.spinning",
            "<x-dc><helmet><style>body{margin:0;background:#123}\
@keyframes turn{from{transform:rotate(15deg)}to{transform:rotate(45deg)}}\
@layer motion{\
#spin{position:absolute;left:50px;top:50px;width:40px;height:40px;background:#fff;\
animation:turn 3s linear infinite !important}\
#fade{position:absolute;left:150px;top:50px;width:40px;height:40px;background:#fff;\
transition:opacity 1s !important}}#fade.on{opacity:0}</style></helmet>\
<div id=\"spin\"></div><div id=\"fade\"></div></x-dc>",
        ),
        (
            "fixture.roboto",
            "<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=Roboto:wght@400;700&amp;display=swap\">\
<x-dc><helmet><style>body{margin:0;font:700 20px Roboto}</style></helmet><div>Roboto</div></x-dc>",
        ),
        (
            "fixture.half",
            "<x-dc><div style=\"position:absolute;left:0;top:0;width:100px;height:80px;background:#000\"></div>\
<div style=\"position:absolute;left:20px;top:10px;width:40px;height:30px;background:#fff\"></div></x-dc>",
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

#[tokio::test]
async fn a_spinning_element_records_its_unrotated_box() {
    let root = tree();
    let (width, opacity) = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        render(
            &page,
            "fixture.spinning",
            &server.board_url("fixture.spinning"),
            Frame { w: 300, h: 200 },
            1.0,
        )
        .await
        .unwrap();
        let width: f64 = page
            .evaluate("document.querySelector('#spin').getBoundingClientRect().width")
            .await
            .unwrap()
            .into_value()
            .unwrap();
        let opacity: String = page
            .evaluate(
                "document.querySelector('#fade').classList.add('on');\
                 getComputedStyle(document.querySelector('#fade')).opacity",
            )
            .await
            .unwrap()
            .into_value()
            .unwrap();
        (width, opacity)
    })
    .await;
    assert_eq!(width, 40.0);
    assert_eq!(opacity, "0");
}

#[tokio::test]
async fn a_google_fonts_link_loads_the_checkers_own_copy_and_reaches_no_other_server() {
    let root = tree();
    let (loaded, requests) = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        render(
            &page,
            "fixture.roboto",
            &server.board_url("fixture.roboto"),
            Frame { w: 300, h: 200 },
            1.0,
        )
        .await
        .unwrap();
        let loaded: bool = page
            .evaluate("document.fonts.check('700 20px Roboto') && [...document.fonts].some(f => f.family.includes('Roboto') && f.status === 'loaded')")
            .await
            .unwrap()
            .into_value()
            .unwrap();
        let requests: Vec<String> = page
            .evaluate("performance.getEntriesByType('resource').map(e => e.name)")
            .await
            .unwrap()
            .into_value()
            .unwrap();
        (loaded, requests)
    })
    .await;
    assert!(loaded, "{requests:?}");
    let outside: Vec<_> = requests
        .iter()
        .filter(|url| !url.starts_with("http://127.0.0.1:"))
        .collect();
    assert!(outside.is_empty(), "{outside:?}");
}

fn runtime_errors(result: Result<Rendered, BoardError>) -> Vec<String> {
    match result {
        Err(BoardError::Runtime { errors, .. }) => errors,
        other => panic!(
            "expected a runtime error, got {:?}",
            other.map(|r| r.png.len())
        ),
    }
}

#[tokio::test]
async fn a_link_to_a_web_font_the_checker_holds_no_copy_of_fails_the_board() {
    let root = tree();
    fixture::write(
        root.path(),
        "design/mocks/fixture.oswald.dc.html",
        &format!(
            "{}<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=Oswald\"><x-dc><div>Oswald</div></x-dc></body></html>",
            fixture::HEAD
        ),
    );
    let mut design = fixture::design();
    design.bundle_css =
        "@import url('https://fonts.googleapis.com/css2?family=Lobster');".to_string();
    let linked = fixture::with_design(root.path(), &fixture::design(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        render(
            &page,
            "fixture.oswald",
            &server.board_url("fixture.oswald"),
            Frame { w: 300, h: 200 },
            1.0,
        )
        .await
    })
    .await;
    let imported = fixture::with_design(root.path(), &design, async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        render(
            &page,
            "fixture.wrapped",
            &server.board_url("fixture.wrapped"),
            Frame { w: 300, h: 200 },
            1.0,
        )
        .await
    })
    .await;
    let linked = runtime_errors(linked);
    assert!(linked.iter().any(|e| e.contains("Oswald")), "{linked:?}");
    let imported = runtime_errors(imported);
    assert!(
        imported.iter().any(|e| e.contains("Lobster")),
        "{imported:?}"
    );
}

/// The picture holds the page: a white box on black, not the flat picture of
/// the browser window that a capture from the page returns.
#[tokio::test]
async fn a_board_picture_holds_the_boards_pixels() {
    let root = tree();
    let rendered = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        render(
            &page,
            "fixture.half",
            &server.board_url("fixture.half"),
            Frame { w: 100, h: 80 },
            2.0,
        )
        .await
        .unwrap()
    })
    .await;
    let image = image::load_from_memory(&rendered.png).unwrap().to_rgb8();
    assert_eq!((image.width(), image.height()), (200, 160));
    assert_eq!(image.get_pixel(80, 50).0, [255, 255, 255]);
    assert_eq!(image.get_pixel(10, 10).0, [0, 0, 0]);
}
