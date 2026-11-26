//! Loading a page that an app's stage serves, and taking its picture.
//!
//! A board loads under virtual time, which stands still while a request is
//! pending. An app's page keeps an event stream open, so its virtual time
//! would never run out: the page loads in real time instead, until its
//! ready selector matches and its fonts are loaded. Then the fixture's steps
//! run, the page's time pauses and its animations stop, as a board's do
//! (`board::redraw`), and the picture is taken.
//!
//! The page gets the checker's own copies of the fonts a board gets: each
//! Google Fonts link it holds, and the SF Mono files of a Mac, are answered
//! from the checker's files (`web_fonts`, `boards::fonts`) in place of the
//! network.

use crate::design::Frame;
use crate::render::chrome::DrawPage;
use crate::render::{board, web_fonts};
use base64::Engine;
use chromiumoxide::cdp::browser_protocol::fetch::{
    EnableParams, EventRequestPaused, FulfillRequestParams, HeaderEntry, RequestPattern,
};
use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
use futures::StreamExt;
use serde::Deserialize;
use std::path::Path;

/// What the `capture` part of a fixture says of taking its shot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Capture {
    /// The selector of an element the page has once it is drawn.
    pub(crate) ready: String,
    /// What the page is given after it is drawn, in order.
    #[serde(default)]
    pub(crate) steps: Vec<Step>,
}

/// One thing the page is given before its picture.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    /// The selector of the element to click: the first it matches.
    pub(crate) click: String,
    /// The selector of an element the page has once the click's effects are
    /// done. A click whose handler changes the page after a fetch or a timer
    /// names one; without it the step ends once the page has drawn the
    /// frames after the click, which shows what a synchronous handler
    /// changed.
    #[serde(default)]
    pub(crate) until: Option<String>,
}

/// The host that serves the SF Mono files to a page: no network answers it,
/// the checker does.
const SF_MONO_HOST: &str = "https://sf-mono.checker.invalid";

/// Loads `url` in <page> in a frame of `frame` points at the page's device
/// scale, in dark mode, runs the steps of <capture> and leaves the page drawn
/// and still, ready for its picture and its elements to be read. Fails when
/// the page is not drawn: a ready selector that matches nothing never ends
/// by itself, so the page's own deadline ends it (`Chrome::with_page_within`),
/// and the failure then says how far the page got.
pub(crate) async fn load(
    page: &DrawPage,
    url: &str,
    frame: Frame,
    capture: &Capture,
    fonts: Option<&Path>,
) -> Result<(), String> {
    let sf_mono = match fonts {
        Some(folder) => Some(
            boards::fonts::font_face_css(folder)
                .map_err(|e| e.text())?
                .replace(
                    boards::fonts::URL_PREFIX,
                    &format!("{SF_MONO_HOST}{}", boards::fonts::URL_PREFIX),
                ),
        ),
        None => None,
    };
    answering_fonts(page, fonts, async {
        board::prepare(page, frame, &capture.ready, None)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(rules) = &sf_mono {
            page.run(AddScriptToEvaluateOnNewDocumentParams::new(style_script(
                rules,
            )))
            .await
            .map_err(|e| e.to_string())?;
        }
        page.goto(url).await.map_err(|e| e.to_string())?;
        // The watch script's promise settles when the ready selector matches
        // and the fonts are loaded.
        page.evaluate("window.__buildsShots.ready")
            .await
            .map_err(|e| e.to_string())?;
        for step in &capture.steps {
            run(page, step).await?;
        }
        board::redraw(page).await.map_err(|e| e.to_string())?;
        // Inline important rules and animations started by script can still
        // add effects; clear their keyframes, as a board's load does.
        page.evaluate("document.getAnimations().forEach(({ effect }) => effect.setKeyframes([]))")
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
}

/// Runs <step> on <page> and returns once its effects are done: the element
/// the step's `until` selector names is there, and the page has drawn two
/// frames and loaded its fonts. A selector that matches nothing never ends
/// by itself, so the page's own deadline ends it (`Chrome::with_page_within`).
async fn run(page: &DrawPage, step: &Step) -> Result<(), String> {
    let selector = serde_json::to_string(&step.click).expect("a string serializes");
    let until = serde_json::to_string(&step.until).expect("an option of a string serializes");
    let clicked: bool = page
        .evaluate(format!(
            "(async () => {{ \
             const target = document.querySelector({selector}); \
             if (!target) return false; \
             const until = {until}; \
             const done = until === null ? Promise.resolve() : new Promise((resolve) => {{ \
             const look = () => {{ if (document.querySelector(until)) {{ watching.disconnect(); resolve(); }} }}; \
             const watching = new MutationObserver(look); \
             watching.observe(document, {{ childList: true, subtree: true, attributes: true }}); \
             look(); }}); \
             target.click(); \
             await done; \
             await new Promise((frame) => requestAnimationFrame(() => requestAnimationFrame(frame))); \
             await document.fonts.ready; \
             return true; }})()"
        ))
        .await
        .map_err(|e| e.to_string())?
        .into_value()
        .map_err(|e| e.to_string())?;
    if clicked {
        Ok(())
    } else {
        Err(format!(
            "the step click {}: no element matches it",
            step.click
        ))
    }
}

/// The script that adds the style <rules> to each page as it is created.
fn style_script(rules: &str) -> String {
    let rules = serde_json::to_string(rules).expect("a string serializes");
    format!(
        "(() => {{ const style = document.createElement('style'); style.textContent = {rules}; \
         const put = () => {{ if (!document.documentElement) return false; \
         document.documentElement.prepend(style); return true; }}; \
         if (!put()) {{ new MutationObserver((_, watching) => {{ \
         if (put()) watching.disconnect(); }}).observe(document, {{ childList: true }}); }} }})();"
    )
}

/// Runs <work> while <page>'s requests for fonts are answered from the
/// checker's own files, and fails when a request asked for what the checker
/// holds no copy of.
async fn answering_fonts<T>(
    page: &DrawPage,
    folder: Option<&Path>,
    work: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let patterns = [
        web_fonts::GOOGLE_PATTERN.to_string(),
        format!("{SF_MONO_HOST}/*"),
    ]
    .map(|url| RequestPattern {
        url_pattern: Some(url),
        resource_type: None,
        request_stage: None,
    });
    page.run(EnableParams {
        patterns: Some(patterns.to_vec()),
        handle_auth_requests: None,
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut paused = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    tokio::pin!(work);
    let out = loop {
        tokio::select! {
            out = &mut work => break out,
            Some(request) = paused.next() => {
                if let Some(problem) = answer(page, &request, folder).await? {
                    problems.push(problem);
                }
            }
        }
    };
    if problems.is_empty() {
        out
    } else {
        Err(problems.join("\n"))
    }
}

/// Answers the request <request> of <page> from the checker's files. Returns
/// what a page asked for that the checker cannot give.
async fn answer(
    page: &DrawPage,
    request: &EventRequestPaused,
    folder: Option<&Path>,
) -> Result<Option<String>, String> {
    let url = &request.request.url;
    let found = match url.strip_prefix(SF_MONO_HOST) {
        Some(path) => sf_mono(folder, path),
        None => web_fonts::answer_google(url),
    };
    let (status, kind, body, problem) = match found {
        Ok(Some((kind, body))) => (200, kind, body, None),
        Ok(None) => (
            404,
            "text/plain",
            format!("not found: {url}").into_bytes(),
            None,
        ),
        Err(why) => {
            let problem = format!("{url}: {why}");
            (
                500,
                "text/plain",
                problem.clone().into_bytes(),
                Some(problem),
            )
        }
    };
    let fulfill = FulfillRequestParams {
        request_id: request.request_id.clone(),
        response_code: status,
        response_headers: Some(vec![
            HeaderEntry::new("Content-Type", kind),
            HeaderEntry::new("Access-Control-Allow-Origin", "*"),
        ]),
        binary_response_headers: None,
        body: Some(
            base64::engine::general_purpose::STANDARD
                .encode(body)
                .into(),
        ),
        response_phrase: None,
    };
    page.run(fulfill).await.map_err(|e| e.to_string())?;
    Ok(problem)
}

/// The SF Mono file the path <path> names, in <folder>.
fn sf_mono(folder: Option<&Path>, path: &str) -> Result<Option<(&'static str, Vec<u8>)>, String> {
    let Some(boards::routes::Route::Font(name)) = boards::routes::route(path) else {
        return Ok(None);
    };
    let Some(folder) = folder else {
        return Ok(None);
    };
    let file = folder.join(name);
    std::fs::read(&file)
        .map(|bytes| Some(("font/otf", bytes)))
        .map_err(|e| format!("cannot read {}: {e}", file.display()))
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
