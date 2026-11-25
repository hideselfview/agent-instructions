//! A static HTTP server on 127.0.0.1 for the boards, answering with the
//! canvas's routes (`boards::routes`): the boards and their parts as
//! siblings, the runtime copy and the design system under
//! `/ds/<namespace>/`, each also under `/storage/`. It also serves React at
//! `/react.js` and `/react-dom.js`, the stylesheets and files of the web fonts
//! under `/web-fonts/` (`web_fonts`), the SF Mono files of Terminal.app under
//! `/fonts/sf-mono/`, and each component's preview page at
//! `/components/<Name>.html` with the design system loaded ahead of its
//! script.

use crate::design::DesignSystem;
use crate::render::{preview, web_fonts};
use boards::routes::{DesignFile, Route, route};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::net::SocketAddr;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What the server serves: the repo's boards, its design system and the
/// runtime copy.
#[derive(Clone, Copy)]
pub(crate) struct Tree<'a> {
    pub(crate) root: &'a Path,
    pub(crate) design: &'a DesignSystem,
    pub(crate) runtime: &'static str,
}

/// The component named by `/components/<Name>.html`.
fn preview_component(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/components/")?.strip_suffix(".html")?;
    boards::preview::is_component_name(name).then_some(name)
}

/// The type and bytes of a file the server gives.
type Found = (&'static str, Vec<u8>);

/// The bytes of <file>; none when it does not exist, and an error that says
/// why when reading it fails.
async fn read(file: &Path) -> Result<Option<Vec<u8>>, String> {
    match tokio::fs::read(file).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", file.display())),
    }
}

/// What <url_path> names, with each Google Fonts link in a board, a preview or the bundle's stylesheet
/// pointing at the server's own copy (`web_fonts`); none when it names
/// nothing, and an error that says why when it names something the server
/// cannot give.
async fn answer(tree: Tree<'_>, url_path: &str) -> Result<Option<Found>, String> {
    let path = url_path.split(['?', '#']).next().unwrap_or_default();
    let javascript = "text/javascript; charset=utf-8";
    match path {
        "/react.js" => return Ok(Some((javascript, preview::REACT.as_bytes().to_vec()))),
        "/react-dom.js" => {
            return Ok(Some((javascript, preview::REACT_DOM.as_bytes().to_vec())));
        }
        _ => {}
    }
    if let Some(rest) = url_path.strip_prefix(web_fonts::URL_PREFIX) {
        return web_fonts::answer(rest);
    }
    if let Some(component) = preview_component(path) {
        let file = tree.root.join(boards::preview::preview_path(component));
        let Some(bytes) = read(&file).await? else {
            return Ok(None);
        };
        let html = String::from_utf8(bytes)
            .map_err(|error| format!("{} is not text: {error}", file.display()))?;
        let page = preview::assemble(component, &html, &tree.design.namespace)
            .map_err(|error| error.to_string())?;
        let page = web_fonts::relink(&page)?;
        return Ok(Some(("text/html; charset=utf-8", page.into_bytes())));
    }
    let Some(route) = route(url_path) else {
        return Ok(None);
    };
    Ok(match route {
        Route::Runtime => Some((javascript, tree.runtime.as_bytes().to_vec())),
        Route::Design(namespace, _) if namespace != tree.design.namespace => None,
        Route::Font(name) => match &tree.design.fonts {
            Some(fonts) => read(&fonts.join(name))
                .await?
                .map(|bytes| ("font/otf", bytes)),
            None => None,
        },
        Route::Design(_, file) => {
            let (kind, text) = match file {
                DesignFile::TokensCss => ("text/css; charset=utf-8", &tree.design.tokens_css),
                DesignFile::BundleCss => ("text/css; charset=utf-8", &tree.design.bundle_css),
                DesignFile::BundleJs => (javascript, &tree.design.bundle_js),
            };
            let text = if file == DesignFile::BundleCss {
                web_fonts::relink(text)?
            } else {
                text.clone()
            };
            Some((kind, text.into_bytes()))
        }
        Route::Mocks(relative) => {
            let file = tree.root.join(boards::MOCKS).join(relative);
            let Some(bytes) = read(&file).await? else {
                return Ok(None);
            };
            let kind = content_type(&file);
            if kind.starts_with("text/html") {
                let text = String::from_utf8(bytes)
                    .map_err(|error| format!("{} is not text: {error}", file.display()))?;
                Some((kind, web_fonts::relink(&text)?.into_bytes()))
            } else {
                Some((kind, bytes))
            }
        }
    })
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

async fn respond(tree: Tree<'_>, mut stream: TcpStream) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buffer).await?;
        if n == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buffer[..n]);
    }
    let head = String::from_utf8_lossy(&request);
    let mut line = head.lines().next().unwrap_or("").split(' ');
    let (method, target) = (line.next().unwrap_or(""), line.next().unwrap_or(""));
    let answered = if method == "GET" || method == "HEAD" {
        answer(tree, target).await
    } else {
        Ok(None)
    };
    let mut why_header = String::new();
    let (status, kind, body) = match answered {
        Ok(Some((kind, body))) => ("200 OK", kind, body),
        Ok(None) => (
            "404 Not Found",
            "text/plain",
            format!("not found: {target}").into_bytes(),
        ),
        Err(why) => {
            crate::log::note(format_args!("cannot answer {target}: {why}"));
            // The page that asked cannot read the body of a stylesheet it
            // failed to load, so the reason also goes in a header, which the
            // render reads (`board::load`).
            let one_line: String = why
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            why_header = format!("{CANNOT_ANSWER}: {one_line}\r\n");
            (
                "500 Internal Server Error",
                "text/plain",
                format!("cannot answer {target}: {why}").into_bytes(),
            )
        }
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{why_header}Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    if method != "HEAD" {
        stream.write_all(&body).await?;
    }
    stream.shutdown().await
}

/// The header that gives why the server answered 500.
pub(crate) const CANNOT_ANSWER: &str = "X-Cannot-Answer";

pub(crate) struct Server {
    address: SocketAddr,
}

impl Server {
    /// The URL of the preview page of `component`.
    pub(crate) fn preview_url(&self, component: &str) -> String {
        format!("http://{}/components/{component}.html", self.address)
    }

    /// The URL of the board `name`.
    pub(crate) fn board_url(&self, name: &str) -> String {
        format!("http://{}/{name}.dc.html", self.address)
    }
}

/// Answers requests from `listener` until it fails; the connections are
/// served together, in this future.
async fn serve(tree: Tree<'_>, listener: TcpListener) -> std::io::Error {
    let mut connections = FuturesUnordered::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => connections.push(respond(tree, stream)),
                Err(error) => return error,
            },
            Some(done) = connections.next() => {
                if let Err(error) = done {
                    crate::log::note(format_args!("serving a request failed: {error}"));
                }
            }
        }
    }
}

/// Serves <tree> on a free port while `work` runs with the
/// server, and stops with it. The requests are answered in this future, so no
/// task outlives the call.
pub(crate) async fn with_server<T>(
    tree: Tree<'_>,
    work: impl AsyncFnOnce(&Server) -> T,
) -> std::io::Result<T> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let server = Server {
        address: listener.local_addr()?,
    };
    tokio::select! {
        out = work(&server) => Ok(out),
        error = serve(tree, listener) => Err(error),
    }
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
