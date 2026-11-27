//! The picture of a web page at a URL, as `builds web picture` takes it: the
//! pinned Chrome loads the page in a window of the width asked for, at two
//! pixels per point, and once the page has loaded and its fonts are in,
//! pictures it when it has settled (`once_settled`).

use crate::render::chrome::{self, ChromeError, DrawPage};
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, Viewport};
use chromiumoxide::page::ScreenshotParams;
use std::time::Duration;
use tokio::time::Instant;

/// The pixels per point a web page is pictured at.
const SCALE: f64 = 2.0;
/// The height of the window a web page loads in, in points.
const WINDOW_HEIGHT: u32 = 900;
/// How long a page's picture stays the same, with no animation running, before
/// the page has settled.
const STILL_FOR: Duration = Duration::from_millis(500);
/// How long a page has to settle before it is pictured as it is.
pub const SETTLE_LIMIT: Duration = Duration::from_secs(10);
/// The pause between two looks at a page that has not settled.
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);
/// Whether a CSS animation, a CSS transition or another animation of the
/// document is running, one that waits out its delay included.
const ANIMATING: &str =
    "document.getAnimations().some((animation) => animation.playState === 'running')";

/// The web page to picture, and how much of it.
#[derive(Debug, PartialEq, Eq)]
pub struct WebPage<'a> {
    pub url: &'a str,
    /// The window's width in CSS pixels.
    pub width: u32,
    /// Picture the page's whole height, not only the window.
    pub full_page: bool,
}

/// The picture of a web page.
pub struct WebPicture {
    pub png: Vec<u8>,
    /// Whether the page settled within `SETTLE_LIMIT`; if not, <png> shows
    /// the page as it was at the limit.
    pub settled: bool,
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

/// The picture of <page>, drawn by the pinned Chrome kept in the cache folder in
/// <folders>, on a page that takes one of the Chrome slots in the build
/// volume <folders> name.
pub fn picture(folders: &boards::Folders, page: &WebPage) -> Result<WebPicture, WebPictureError> {
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
async fn draw(tab: &DrawPage, page: &WebPage<'_>) -> Result<WebPicture, String> {
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
    once_settled(tab, page).await
}

/// Pictures <page>, loaded in <tab>, until it has settled: no animation is
/// running and its pictures have stayed the same for `STILL_FOR`. A picture
/// that stays the same shows no change of layout or paint, whatever makes
/// the page change, such as a script that draws each frame of a fade. A page
/// that has not settled at `SETTLE_LIMIT` gives the last picture taken.
async fn once_settled(tab: &DrawPage, page: &WebPage<'_>) -> Result<WebPicture, String> {
    let limit = Instant::now() + SETTLE_LIMIT;
    // The picture the page has shown with no animation running, and when it
    // was first taken.
    let mut still: Option<(Instant, Vec<u8>)> = None;
    loop {
        let animating: bool = tab
            .evaluate(ANIMATING)
            .await
            .map_err(|e| e.to_string())?
            .into_value()
            .map_err(|e| e.to_string())?;
        let taken = Instant::now();
        let png = screenshot(tab, page).await?;
        let since = match &still {
            Some((since, before)) if *before == png => *since,
            _ => taken,
        };
        let settled = !animating && taken - since >= STILL_FOR;
        if settled || Instant::now() >= limit {
            return Ok(WebPicture { png, settled });
        }
        still = (!animating).then_some((since, png));
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// The PNG of <page> as <tab> shows it now.
async fn screenshot(tab: &DrawPage, page: &WebPage<'_>) -> Result<Vec<u8>, String> {
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
