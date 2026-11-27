//! The picture of a web page at a URL, as `builds web picture` takes it: the
//! pinned Chrome loads the page in a window of the width asked for, at two
//! pixels per point, and pictures it once the page has loaded and its fonts
//! are in.

use crate::render::chrome::{self, ChromeError, DrawPage};
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, Viewport};
use chromiumoxide::page::ScreenshotParams;

/// The pixels per point a web page is pictured at.
const SCALE: f64 = 2.0;
/// The height of the window a web page loads in, in points.
const WINDOW_HEIGHT: u32 = 900;

/// The web page to picture, and how much of it.
#[derive(Debug, PartialEq, Eq)]
pub struct WebPage<'a> {
    pub url: &'a str,
    /// The window's width in CSS pixels.
    pub width: u32,
    /// Picture the page's whole height, not only the window.
    pub full_page: bool,
}

/// Why there is no picture of a web page.
#[derive(Debug)]
pub enum WebPictureError {
    /// Chrome ran and printed no debugging address in its deadline: on macOS,
    /// a window server that does not answer.
    NoAddress(String),
    /// Chrome did not start for another reason.
    Chrome(String),
    /// Chrome started and did not picture the page.
    Page(String),
}

/// The PNG of <page>, drawn by the pinned Chrome kept in the cache folder in
/// <folders>, on a page that takes one of the Chrome slots in the build
/// volume <folders> name.
pub fn picture(folders: &boards::Folders, page: &WebPage) -> Result<Vec<u8>, WebPictureError> {
    let drawn = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| WebPictureError::Chrome(e.to_string()))?
        .block_on(chrome::with_chrome(folders, SCALE, async |chrome| {
            chrome
                .with_page(page.url, async |tab| draw(tab, page).await)
                .await
        }));
    match drawn {
        Ok(drawn) => drawn.map_err(WebPictureError::Page),
        Err(error @ ChromeError::NoAddress(_)) => {
            Err(WebPictureError::NoAddress(error.to_string()))
        }
        Err(error) => Err(WebPictureError::Chrome(error.to_string())),
    }
}

/// Loads <page> in <tab> and pictures it.
async fn draw(tab: &DrawPage, page: &WebPage<'_>) -> Result<Vec<u8>, String> {
    // A device scale factor of 0 leaves the window's real one, which the
    // browser was started with.
    tab.run(SetDeviceMetricsOverrideParams::new(
        i64::from(page.width),
        i64::from(WINDOW_HEIGHT),
        0.0,
        false,
    ))
    .await
    .map_err(|e| e.to_string())?;
    // Returns once the page's load event has fired.
    tab.goto(page.url).await.map_err(|e| e.to_string())?;
    tab.evaluate("document.fonts.ready.then(() => true)")
        .await
        .map_err(|e| e.to_string())?;
    let mut shot = ScreenshotParams::builder().format(CaptureScreenshotFormat::Png);
    if page.full_page {
        let metrics = tab.layout_metrics().await.map_err(|e| e.to_string())?;
        // A clip beyond the window keeps the window's size, so that a box
        // sized by the window's height (`100vh`) stays as tall as it was.
        shot = shot
            .clip(Viewport {
                x: 0.0,
                y: 0.0,
                width: f64::from(page.width),
                height: metrics.css_content_size.height,
                scale: 1.0,
            })
            .capture_beyond_viewport(true);
    }
    tab.screenshot(shot.build())
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "web_picture_tests.rs"]
mod tests;
