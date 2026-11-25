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
use crate::shots::Bounds;
use chromiumoxide::Page;
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

async fn grant(page: &Page, budget: f64) -> Result<(), BoardError> {
    let mut policy =
        SetVirtualTimePolicyParams::new(VirtualTimePolicy::PauseIfNetworkFetchesPending);
    policy.budget = Some(budget);
    page.execute(policy).await.map_err(protocol)?;
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
  new Promise((resolve) => {{
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

/// The element that says a canvas board is drawn: the runtime's root.
pub(crate) const BOARD_MOUNTED: &str = "#dc-root";

/// Draws the board at `url` in a frame of `frame` points at `scale` pixels per
/// point, in dark mode. The picture comes from the page itself and not from a
/// copy of the compositor's latest frame, which holds the antialiased edge of a
/// rounded clip blended once or twice by how much the frames before it
/// damaged (`preview::capture`).
pub(crate) async fn render(
    page: &Page,
    board: &str,
    url: &str,
    frame: Frame,
    scale: f64,
) -> Result<Rendered, BoardError> {
    load(page, board, url, frame, scale, BOARD_MOUNTED, None).await?;
    let png = page
        .screenshot(
            ScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .from_surface(false)
                .build(),
        )
        .await
        .map_err(protocol)?;
    Ok(Rendered {
        png,
        frame: Bounds {
            x: 0.0,
            y: 0.0,
            width: f64::from(frame.w),
            height: f64::from(frame.h),
        },
    })
}

/// Loads `url` in a frame of `frame` points at `scale` pixels per point, in
/// dark mode, and returns once its virtual time has run out with the page
/// drawn: the element <mounted> selects is there and the fonts are loaded.
/// The page's `<body>` gets the class <body_class>, if any, when it is created.
pub(crate) async fn load(
    page: &Page,
    board: &str,
    url: &str,
    frame: Frame,
    scale: f64,
    mounted: &str,
    body_class: Option<&str>,
) -> Result<(), BoardError> {
    page.execute(SetDeviceMetricsOverrideParams::new(
        i64::from(frame.w),
        i64::from(frame.h),
        scale,
        false,
    ))
    .await
    .map_err(protocol)?;
    page.execute(SetEmulatedMediaParams {
        media: None,
        features: Some(vec![MediaFeature::new("prefers-color-scheme", "dark")]),
    })
    .await
    .map_err(protocol)?;
    page.execute(AddScriptToEvaluateOnNewDocumentParams::new(watch_script(
        mounted, body_class,
    )))
    .await
    .map_err(protocol)?;
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
