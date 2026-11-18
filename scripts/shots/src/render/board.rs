//! Loading a board in Chrome and taking its picture.
//!
//! The page loads under virtual time with a fixed budget, so animations stop
//! at the same moment on every run; then the runtime's root, the fonts and the
//! pending requests are waited for, granting more virtual time between looks.
//! A runtime error in the console fails the board.

use crate::design::Frame;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::emulation::{
    EventVirtualTimeBudgetExpired, MediaFeature, SetDeviceMetricsOverrideParams,
    SetEmulatedMediaParams, SetVirtualTimePolicyParams, VirtualTimePolicy,
};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::cdp::js_protocol::runtime::{
    ConsoleApiCalledType, EventConsoleApiCalled, EventExceptionThrown, RemoteObject,
};
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use std::time::Duration;

/// Virtual milliseconds the page runs for after navigation starts.
const LOAD_BUDGET_MS: f64 = 2000.0;
/// Virtual milliseconds granted per further look when the page is not ready.
const EXTRA_BUDGET_MS: f64 = 500.0;
const EXTRA_LOOKS: usize = 20;
/// Real time one budget may take to run out.
const BUDGET_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub(crate) enum BoardError {
    #[error("Chrome failed: {0}")]
    Protocol(String),
    #[error("the board {board} was not ready after {looks} looks: {state}")]
    NotReady {
        board: String,
        looks: usize,
        state: String,
    },
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

async fn expired(
    expired: &mut (impl futures::Stream<Item = std::sync::Arc<EventVirtualTimeBudgetExpired>> + Unpin),
) -> Result<(), BoardError> {
    tokio::time::timeout(BUDGET_TIMEOUT, expired.next())
        .await
        .map_err(|elapsed| {
            BoardError::Protocol(format!("virtual time never ran out: {elapsed}"))
        })?;
    Ok(())
}

/// What a canvas board reports about being ready: `ready` or the first thing
/// missing.
pub(crate) const BOARD_READY: &str = r#"(() => {
  if (!document.querySelector('#dc-root')) return 'no #dc-root';
  if (document.fonts.status !== 'loaded') return 'fonts ' + document.fonts.status;
  return 'ready';
})()"#;

/// Draws the board at `url` in a frame of `frame` points at `scale` pixels per
/// point, in dark mode.
pub(crate) async fn render(
    page: &Page,
    board: &str,
    url: &str,
    frame: Frame,
    scale: f64,
) -> Result<Rendered, BoardError> {
    load(page, board, url, frame, scale, BOARD_READY).await?;
    let png = page
        .screenshot(
            ScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .build(),
        )
        .await
        .map_err(protocol)?;
    Ok(Rendered { png })
}

/// Loads `url` in a frame of `frame` points at `scale` pixels per point, in
/// dark mode, and returns once `ready` (an expression that answers `ready` or
/// what is missing) says the page is drawn and its virtual time has run out.
pub(crate) async fn load(
    page: &Page,
    board: &str,
    url: &str,
    frame: Frame,
    scale: f64,
    ready_expression: &str,
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

    page.goto(url).await.map_err(protocol)?;
    grant(page, LOAD_BUDGET_MS).await?;
    expired(&mut budget).await?;

    let mut state = String::new();
    let mut ready = false;
    for _ in 0..=EXTRA_LOOKS {
        state = page
            .evaluate(ready_expression)
            .await
            .map_err(protocol)?
            .into_value::<String>()
            .map_err(protocol)?;
        if state == "ready" {
            ready = true;
            break;
        }
        grant(page, EXTRA_BUDGET_MS).await?;
        expired(&mut budget).await?;
    }

    let mut errors = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(0), console.next()).await
    {
        let message = event.args.iter().map(text).collect::<Vec<_>>().join(" ");
        if is_runtime_error(&event.r#type, &message) {
            errors.push(message);
        }
    }
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(0), thrown.next()).await
    {
        let details = &event.exception_details;
        let detail = details.exception.as_ref().map(text).unwrap_or_default();
        errors.push(format!("uncaught: {} {detail}", details.text));
    }
    if !errors.is_empty() {
        return Err(BoardError::Runtime {
            board: board.to_string(),
            errors,
        });
    }
    if !ready {
        return Err(BoardError::NotReady {
            board: board.to_string(),
            looks: EXTRA_LOOKS,
            state,
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
