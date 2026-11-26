//! Loading a board in Chrome and taking its picture.
//!
//! The loader suppresses stylesheet motion during loading and removes any
//! remaining animation effects before the picture and element reader measure
//! the page.
//!
//! The page loads under virtual time with a fixed budget. Virtual time stands
//! still while a request is pending. A script runs before the page's own
//! scripts and watches for the caller's readiness selector and for
//! `document.fonts.ready`. The selector names the canvas runtime's `#dc-root`,
//! the preview's body for a full page, or the preview's first row for a row
//! capture. When the budget runs out, a page that did not finish drawing or
//! wrote a runtime error to the console fails.

use crate::design::Frame;
use crate::render::chrome::DrawPage;
use crate::shots::Bounds;
use chromiumoxide::cdp::browser_protocol::emulation::{
    EventVirtualTimeBudgetExpired, MediaFeature, SetDeviceMetricsOverrideParams,
    SetEmulatedMediaParams, SetVirtualTimePolicyParams, VirtualTimePolicy,
};
use chromiumoxide::cdp::browser_protocol::network::EventResponseReceived;
use chromiumoxide::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, CaptureScreenshotFormat,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    ConsoleApiCalledType, EventConsoleApiCalled, EventExceptionThrown, RemoteObject,
};
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use std::time::Duration;

/// Virtual milliseconds the page runs for after navigation starts.
const LOAD_BUDGET_MS: f64 = 2000.0;
/// Real time the budget may take to run out.
const BUDGET_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub(crate) enum BoardError {
    #[error("Chrome failed: {0}")]
    Protocol(String),
    #[error(
        "the board {board} was not drawn when its {LOAD_BUDGET_MS} virtual milliseconds ran out: {state}"
    )]
    NotReady { board: String, state: String },
    #[error("the board {board} raised runtime errors:\n{}", .errors.join("\n"))]
    Runtime { board: String, errors: Vec<String> },
}

pub(crate) fn protocol(error: impl std::fmt::Display) -> BoardError {
    BoardError::Protocol(error.to_string())
}

/// A board's picture.
pub(crate) struct Rendered {
    /// The PNG at `scale` pixels per point.
    pub(crate) png: Vec<u8>,
    /// The captured rectangle in viewport coordinates, in points.
    pub(crate) frame: Bounds,
}

fn text(object: &RemoteObject) -> String {
    match (&object.value, &object.description) {
        (Some(serde_json::Value::String(s)), _) => s.clone(),
        (Some(value), _) => value.to_string(),
        (None, Some(description)) => description.clone(),
        (None, None) => String::new(),
    }
}

/// Whether a console message is one the runtime raises for a broken board.
fn is_runtime_error(kind: &ConsoleApiCalledType, message: &str) -> bool {
    match kind {
        ConsoleApiCalledType::Error => true,
        ConsoleApiCalledType::Warning => message.starts_with("[dc-runtime]"),
        _ => false,
    }
}

async fn grant(page: &DrawPage, budget: f64) -> Result<(), BoardError> {
    let mut policy =
        SetVirtualTimePolicyParams::new(VirtualTimePolicy::PauseIfNetworkFetchesPending);
    policy.budget = Some(budget);
    page.run(policy).await.map_err(protocol)?;
    Ok(())
}

/// The script a page runs before its own: it records in
/// `window.__buildsShots.state` how far the page got toward drawn, waiting on
/// the element <mounted> selects to appear and then on the page's fonts, and
/// in `globals` the names the window holds before the page's scripts run. Its
/// `components()` gives the design system's components by their values,
/// which the board's reader (read.rs) and the reader of the components a page
/// draws (instances.rs) look React's fibers up in.
fn watch_script(mounted: &str, body_class: Option<&str>) -> String {
    let selector = serde_json::to_string(mounted).expect("a string serializes");
    let class = serde_json::to_string(&body_class).expect("an option of a string serializes");
    format!(
        r#"(() => {{
  const watch = {{
    state: 'nothing matches ' + {selector},
    globals: new Set(Object.getOwnPropertyNames(window)),
  }};
  // The design system's components by their values: each function or
  // memo/forwardRef object that a global the page's scripts added holds
  // (`Bae.MacButton` is MacButton). React's globals, and the double-underscore
  // ones of the runtime and of this program, hold none. Read once, after the
  // page's scripts ran.
  let found = null;
  watch.components = () => {{
    if (found) return found;
    const map = new Map();
    for (const name of Object.getOwnPropertyNames(window)) {{
      if (watch.globals.has(name) || name.startsWith('__')
        || name === 'React' || name === 'ReactDOM') continue;
      const holder = window[name];
      if (!holder || typeof holder !== 'object') continue;
      for (const [key, value] of Object.entries(holder)) {{
        const component = typeof value === 'function'
          || (value && typeof value === 'object' && value.$$typeof
            && ('render' in value || 'type' in value));
        if (component && !map.has(value)) map.set(value, key);
      }}
    }}
    found = map;
    return map;
  }};
  window.__buildsShots = watch;
  // Important declarations in the first cascade layer take precedence over
  // important declarations in the page's stylesheets.
  const freeze = document.createElement('style');
  freeze.textContent = '@layer {{ *, *::before, *::after {{ '
    + 'animation: none !important; transition: none !important; }} }}';
  // Runs <step> now, or as soon as the document changes enough for it to
  // return true.
  const whenPossible = (step) => {{
    if (step()) return;
    const waiting = new MutationObserver(() => {{
      if (step()) waiting.disconnect();
    }});
    waiting.observe(document, {{ childList: true, subtree: true }});
  }};
  whenPossible(() => {{
    if (!document.documentElement) return false;
    document.documentElement.prepend(freeze);
    return true;
  }});
  // A class the page's <body> carries from the moment it exists, so the
  // fonts its styles name load within the page's virtual time budget, as they
  // do for a page whose HTML holds the class.
  const bodyClass = {class};
  if (bodyClass) {{
    whenPossible(() => {{
      if (!document.body) return false;
      document.body.classList.add(bodyClass);
      return true;
    }});
  }}
  // Settles once the page is drawn, which a page that loads in real time
  // waits on.
  watch.ready = new Promise((resolve) => {{
    const look = () => {{
      if (!document.querySelector({selector})) return;
      observer.disconnect();
      resolve();
    }};
    const observer = new MutationObserver(look);
    observer.observe(document, {{ childList: true, subtree: true }});
  }})
    .then(() => {{
      watch.state = 'fonts loading';
      return document.fonts.ready;
    }})
    .then(() => {{
      watch.state = 'ready';
    }});
}})();"#
    )
}

/// Hides and shows the page's body three times, with two frames after each
/// step. Chrome draws the frame it takes a picture of over the frames before
/// it, and a rounded corner's antialiased edge comes out of that differently
/// by how many frames drew it. After this, every draw of a page has drawn it
/// the same number of times. The frames need the page's virtual time to run.
/// Unlike `display: none`, hiding with `visibility` leaves the page's
/// animations running, so the promises it tracks them with stay as they are.
const REDRAW: &str = "(async () => {
  const frames = () => new Promise(
    (done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  for (let turn = 0; turn < 3; turn++) {
    document.body.style.visibility = 'hidden';
    await frames();
    document.body.style.visibility = '';
    await frames();
  }
})()";

pub(crate) async fn redraw(page: &DrawPage) -> Result<(), BoardError> {
    let running = SetVirtualTimePolicyParams::new(VirtualTimePolicy::Advance);
    page.run(running).await.map_err(protocol)?;
    page.evaluate(REDRAW).await.map_err(protocol)?;
    let paused = SetVirtualTimePolicyParams::new(VirtualTimePolicy::Pause);
    page.run(paused).await.map_err(protocol)?;
    Ok(())
}

/// The element that says a canvas board is drawn: the runtime's root.
pub(crate) const BOARD_MOUNTED: &str = "#dc-root";

/// Draws the board at `url` in a frame of `frame` points at the page's device
/// scale, in dark mode. The picture is the compositor's latest frame. A capture
/// from the page (`from_surface(false)`) returns a flat dark picture of the
/// browser window with none of the page in it.
pub(crate) async fn render(
    page: &DrawPage,
    board: &str,
    url: &str,
    frame: Frame,
) -> Result<Rendered, BoardError> {
    load(page, board, url, frame, BOARD_MOUNTED, None).await?;
    let png = screenshot(page).await?;
    Ok(Rendered {
        png,
        frame: frame_bounds(frame),
    })
}

/// The rectangle a board of <frame> points covers, from its top-left corner.
pub(crate) fn frame_bounds(frame: Frame) -> Bounds {
    Bounds {
        x: 0.0,
        y: 0.0,
        width: f64::from(frame.w),
        height: f64::from(frame.h),
    }
}

/// The picture of the page `load` drew: the compositor's latest frame.
pub(crate) async fn screenshot(page: &DrawPage) -> Result<Vec<u8>, BoardError> {
    page.screenshot(
        ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .build(),
    )
    .await
    .map_err(protocol)
}

/// Sets <page> up for a page to load in a frame of `frame` points at the
/// page's device scale, in dark mode, with the script that watches for the
/// element <mounted> selects and for the fonts; the page's `<body>` gets the
/// class <body_class>, if any, when it is created.
pub(crate) async fn prepare(
    page: &DrawPage,
    frame: Frame,
    mounted: &str,
    body_class: Option<&str>,
) -> Result<(), BoardError> {
    // A device scale factor of 0 leaves the window's real one, which the browser
    // was started with (`chrome::with_chrome`).
    page.run(SetDeviceMetricsOverrideParams::new(
        i64::from(frame.w),
        i64::from(frame.h),
        0.0,
        false,
    ))
    .await
    .map_err(protocol)?;
    page.run(SetEmulatedMediaParams {
        media: None,
        features: Some(vec![MediaFeature::new("prefers-color-scheme", "dark")]),
    })
    .await
    .map_err(protocol)?;
    page.run(AddScriptToEvaluateOnNewDocumentParams::new(watch_script(
        mounted, body_class,
    )))
    .await
    .map_err(protocol)?;
    Ok(())
}

/// Loads `url` in a frame of `frame` points at the page's device scale, in
/// dark mode, and returns once its virtual time has run out with the page
/// drawn: the element <mounted> selects is there and the fonts are loaded.
/// The page's `<body>` gets the class <body_class>, if any, when it is created.
pub(crate) async fn load(
    page: &DrawPage,
    board: &str,
    url: &str,
    frame: Frame,
    mounted: &str,
    body_class: Option<&str>,
) -> Result<(), BoardError> {
    prepare(page, frame, mounted, body_class).await?;
    let mut console = page
        .event_listener::<EventConsoleApiCalled>()
        .await
        .map_err(protocol)?;
    let mut thrown = page
        .event_listener::<EventExceptionThrown>()
        .await
        .map_err(protocol)?;
    let mut budget = page
        .event_listener::<EventVirtualTimeBudgetExpired>()
        .await
        .map_err(protocol)?;

    // The server answers 500 for what it cannot give, such as a link to a web
    // font it holds no copy of; the page goes on without it, so a 500 fails the
    // board. chromiumoxide has enabled the Network domain by now.
    let mut answered = page
        .event_listener::<EventResponseReceived>()
        .await
        .map_err(protocol)?;

    page.goto(url).await.map_err(protocol)?;
    grant(page, LOAD_BUDGET_MS).await?;

    // Chrome sends a page's events in order, so every console message and
    // exception the page raised before its budget ran out comes ahead of the
    // budget's event; `biased` takes them first.
    let mut errors = Vec::new();
    let deadline = tokio::time::sleep(BUDGET_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            Some(event) = console.next() => {
                let message = event.args.iter().map(text).collect::<Vec<_>>().join(" ");
                if is_runtime_error(&event.r#type, &message) {
                    errors.push(message);
                }
            }
            Some(event) = thrown.next() => {
                let details = &event.exception_details;
                let detail = details.exception.as_ref().map(text).unwrap_or_default();
                errors.push(format!("uncaught: {} {detail}", details.text));
            }
            Some(event) = answered.next() => {
                let response = &event.response;
                if response.status >= 500 {
                    let why = response
                        .headers
                        .inner()
                        .as_object()
                        .into_iter()
                        .flatten()
                        .find(|(name, _)| name.eq_ignore_ascii_case(crate::render::serve::CANNOT_ANSWER))
                        .and_then(|(_, why)| why.as_str());
                    errors.push(match why {
                        Some(why) => format!("{} answered {}: {why}", response.url, response.status),
                        None => format!(
                            "{} answered {} and gave no {} header",
                            response.url,
                            response.status,
                            crate::render::serve::CANNOT_ANSWER
                        ),
                    });
                }
            }
            Some(_) = budget.next() => break,
            () = &mut deadline => {
                return Err(BoardError::Protocol(format!(
                    "the board's virtual time had not run out after {} real seconds",
                    BUDGET_TIMEOUT.as_secs()
                )));
            }
        }
    }
    if !errors.is_empty() {
        return Err(BoardError::Runtime {
            board: board.to_string(),
            errors,
        });
    }
    let state = page
        .evaluate("window.__buildsShots.state")
        .await
        .map_err(protocol)?
        .into_value::<String>()
        .map_err(protocol)?;
    if state != "ready" {
        return Err(BoardError::NotReady {
            board: board.to_string(),
            state,
        });
    }
    redraw(page).await?;
    // Inline important rules and animations started by script can still add
    // effects. Clear their keyframes without cancelling the animations,
    // which would reject any promises the page uses to track them.
    page.evaluate("document.getAnimations().forEach(({ effect }) => effect.setKeyframes([]))")
        .await
        .map_err(protocol)?;
    Ok(())
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
