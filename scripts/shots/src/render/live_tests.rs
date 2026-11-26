use super::*;
use crate::render::fixture;

const FRAME: Frame = Frame { w: 300, h: 200 };

fn capture() -> Capture {
    Capture {
        ready: "#text".to_string(),
        steps: Vec::new(),
    }
}

const ROBOTO: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=Roboto:wght@400;700&amp;display=swap\">\
<style>body{margin:0;font:700 20px Roboto}</style></head><body><div id=\"text\">Roboto</div></body></html>";

const UNHELD: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=Lato\">\
<style>body{margin:0;font:700 20px Lato}</style></head><body><div id=\"text\">Lato</div></body></html>";

const SF_MONO: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<style>body{margin:0;font:400 20px 'SF Mono'}</style></head><body><div id=\"text\">SF Mono</div></body></html>";

#[tokio::test]
async fn a_google_fonts_link_of_a_live_page_loads_the_checkers_copy_and_reaches_no_network() {
    let root = tempfile::tempdir().unwrap();
    let url = fixture::serve_page(ROBOTO);
    let (loaded, requests) = fixture::with_browser_at(root.path(), 2.0, async |_, chrome| {
        let page = chrome.new_page().await.unwrap();
        load(&page, &url, FRAME, &capture(), None).await.unwrap();
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
        .filter(|url| !url.starts_with("https://fonts.googleapis.com/"))
        .collect();
    assert!(outside.is_empty(), "{outside:?}");
}

#[tokio::test]
async fn a_link_to_a_family_the_checker_holds_no_copy_of_fails_the_page() {
    let root = tempfile::tempdir().unwrap();
    let url = fixture::serve_page(UNHELD);
    let error = fixture::with_browser_at(root.path(), 2.0, async |_, chrome| {
        let page = chrome.new_page().await.unwrap();
        load(&page, &url, FRAME, &capture(), None).await
    })
    .await
    .unwrap_err();
    assert!(error.contains("Lato"), "{error}");
}

#[tokio::test]
async fn the_sf_mono_files_of_the_mac_draw_a_page_that_names_the_family() {
    let root = tempfile::tempdir().unwrap();
    let url = fixture::serve_page(SF_MONO);
    let fonts = Path::new(boards::fonts::TERMINAL_FONTS);
    let loaded: bool = fixture::with_browser_at(root.path(), 2.0, async |_, chrome| {
        let page = chrome.new_page().await.unwrap();
        load(&page, &url, FRAME, &capture(), Some(fonts))
            .await
            .unwrap();
        page.evaluate(
            "[...document.fonts].some(f => f.family.includes('SF Mono') && f.status === 'loaded')",
        )
        .await
        .unwrap()
        .into_value()
        .unwrap()
    })
    .await;
    assert!(loaded);
}
