//! The page's server: a few GET routes on 127.0.0.1, one POST route, and one
//! stream of server-sent events per open page. A request whose Host isn't
//! this server's own address is refused, so a page from elsewhere that
//! resolves its name to 127.0.0.1 can't read these; a POST whose Origin isn't
//! the page's own is refused too, so a web page can't trigger it. (A browser
//! sends the real Origin on every cross-site POST and a page can't change it,
//! so no token is needed; a POST with no Origin, which a browser never sends,
//! is refused as well.)
//!
//!     /             the page (page.html, built into the binary)
//!     /api/state    the whole state as JSON
//!     /api/health   health, judged now
//!     POST /api/queue  `{"action": "top"|"takeover", "repo", "pid"}`: the
//!                   same as `builds queue <repo> top <pid>` and
//!                   `builds takeover <repo> <pid>`
//!     POST /api/build-app  `{"repo", "lane": "mac"|"android", "ref"?}`: starts
//!                   `builds build-app <repo> <lane> [<ref>]` as a child of
//!                   the daemon (runs.rs), 409 when the lane has a build
//!                   queued or running; the runs are in `/api/state` and its
//!                   events, with their output
//!     /api/lists    the lists JSON (lists/view.rs, plus each running item's
//!                   claim facts), as the `lists` event carries it
//!     /api/hooks    the hook runs' figures and latest runs (hook_stats.rs),
//!                   as the `hooks` event carries them
//!     /api/hooks/impact?project=<project>&change=<rev>[&repo=<repo>]
//!                   the project's hook runs split by whether their tree has
//!                   that change (hook_impact.rs), read now
//!     /design/<design id>
//!                   the review page of an open design change, drawn now
//!     /design/<design id>/board/<n>
//!                   the <n>th board of that page, drawn now from the
//!                   change's worktree; the page's frames load these
//!     /events       `state` events: the whole state on connect and each time
//!                   it changes, at most four a second; `lists` events: the
//!                   lists JSON on connect and each time it changes, never
//!                   with a `state` event that only repeats the processes

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::model::Model;

const PAGE: &str = include_str!("page.html");
/// The most a POST's body may hold.
const MAX_BODY: usize = 4096;
/// The least time between two events on one stream: pacing for the page.
const EVENT_GAP: Duration = Duration::from_millis(250);

pub fn serve(listener: TcpListener, port: u16, model: Arc<Model>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let model = model.clone();
        std::thread::spawn(move || {
            if let Err(error) = handle(stream, port, &model)
                && !matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                )
            {
                eprintln!("builds: serving a request: {error}");
            }
        });
    }
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn handle(mut stream: TcpStream, port: u16, model: &Model) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let (mut host, mut origin, mut length) = (None, None, 0usize);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("host") {
                host = Some(value.to_string());
            } else if name.eq_ignore_ascii_case("origin") {
                origin = Some(value.to_string());
            } else if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(usize::MAX);
            }
        }
    }
    // Read the body before any answer, so a refusal isn't a reset connection.
    let mut body = vec![0; if length <= MAX_BODY { length } else { 0 }];
    reader.read_exact(&mut body)?;
    let mut parts = request.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let allowed = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !host.is_some_and(|host| allowed.contains(&host)) {
        return respond(
            &mut stream,
            "403 Forbidden",
            "text/plain; charset=utf-8",
            b"only for this machine's own browser\n",
        );
    }
    let path = target.split('?').next().unwrap_or("");
    if method == "POST" && (path == "/api/queue" || path == "/api/build-app") {
        if !from_own_page(origin.as_deref(), port) {
            return respond(
                &mut stream,
                "403 Forbidden",
                "text/plain; charset=utf-8",
                b"only from the page itself\n",
            );
        }
        if length > MAX_BODY {
            return respond(
                &mut stream,
                "413 Payload Too Large",
                "text/plain; charset=utf-8",
                b"too large\n",
            );
        }
        if path == "/api/build-app" {
            return build_app(&mut stream, model, &body);
        }
        return json(
            &mut stream,
            queue_action(model, &body).map(|()| b"{}".to_vec()),
        );
    }
    if method != "GET" {
        return respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"GET only, and POST /api/queue and /api/build-app\n",
        );
    }
    match path {
        "/" => respond(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        ),
        "/api/state" => json(
            &mut stream,
            model
                .page()
                .map(|page| serde_json::to_vec(&page).expect("the state serializes")),
        ),
        "/api/health" => json(
            &mut stream,
            match model.held_health() {
                Some(health) => Ok(serde_json::to_vec(&health).expect("health serializes")),
                None => Err(crate::fail::Fail::Message(
                    "the daemon has not read the processes yet".into(),
                )),
            },
        ),
        "/api/lists" => match model.lists_json() {
            Ok(lists) => respond(&mut stream, "200 OK", "application/json", lists.as_bytes()),
            Err(error) => json(&mut stream, Err(error)),
        },
        "/api/hooks" => respond(
            &mut stream,
            "200 OK",
            "application/json",
            model.hooks_json().as_bytes(),
        ),
        "/api/hooks/impact" => {
            let query = target.split_once('?').map_or("", |(_, query)| query);
            let (project, change) = (query_value(query, "project"), query_value(query, "change"));
            match (project, change) {
                (Some(project), Some(change)) => {
                    let body = model.hook_impact_json(
                        &project,
                        &change,
                        query_value(query, "repo").as_deref(),
                    );
                    respond(&mut stream, "200 OK", "application/json", body.as_bytes())
                }
                _ => respond(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    b"{\"error\":\"needs project and change\"}",
                ),
            }
        }
        "/events" => events(stream, model),
        _ if path.starts_with("/design/") => {
            let drawn = design_route(&path["/design/".len()..]).and_then(|route| match route {
                DesignRoute::Page(id) => model.design_page(id),
                DesignRoute::Board(id, n) => model.design_board(id, n),
            });
            match drawn {
                Some(Ok(page)) => respond(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=utf-8",
                    page.as_bytes(),
                ),
                Some(Err(error)) => respond(
                    &mut stream,
                    "500 Internal Server Error",
                    "text/plain; charset=utf-8",
                    format!("{}\n", error.text()).as_bytes(),
                ),
                None => respond(
                    &mut stream,
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    b"no such open design change or board\n",
                ),
            }
        }
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found\n",
        ),
    }
}

/// The value of <name> in a query string, with `%XX` and `+` decoded.
fn query_value(query: &str, name: &str) -> Option<String> {
    let raw = query.split('&').find_map(|pair| {
        pair.split_once('=')
            .filter(|(key, _)| *key == name)
            .map(|(_, value)| value)
    })?;
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'+' => out.push(b' '),
            b'%' if bytes.len() >= at + 3 => {
                let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                at += 2;
            }
            byte => out.push(byte),
        }
        at += 1;
    }
    String::from_utf8(out).ok()
}

/// What a `/design/...` path asks for.
#[derive(Debug, PartialEq, Eq)]
enum DesignRoute {
    /// The review page of design change <id>.
    Page(u32),
    /// The <n>th board of its review page.
    Board(u32, usize),
}

/// <rest> is the path after `/design/`: `<id>` or `<id>/board/<n>`.
fn design_route(rest: &str) -> Option<DesignRoute> {
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [id] => Some(DesignRoute::Page(id.parse().ok()?)),
        [id, "board", n] => Some(DesignRoute::Board(id.parse().ok()?, n.parse().ok()?)),
        _ => None,
    }
}

/// Does the Origin header name this server's own page?
fn from_own_page(origin: Option<&str>, port: u16) -> bool {
    let own = [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ];
    origin.is_some_and(|origin| own.iter().any(|own| own == origin))
}

#[derive(serde::Deserialize)]
struct BuildRequest {
    repo: String,
    lane: String,
    #[serde(rename = "ref")]
    reference: Option<String>,
}

/// Starts a build-app run, as `builds build-app` (runs.rs). 400 for a
/// request that names no such thing, 409 for a lane that is already building,
/// 500 for a build that could not be started.
fn build_app(stream: &mut TcpStream, model: &Model, body: &[u8]) -> std::io::Result<()> {
    use super::runs::StartError;
    let request: BuildRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => {
            let message =
                serde_json::json!({ "error": format!("the request: {error}") }).to_string();
            return respond(
                stream,
                "400 Bad Request",
                "application/json",
                message.as_bytes(),
            );
        }
    };
    match model
        .runs
        .start_build(&request.repo, &request.lane, request.reference.as_deref())
    {
        Ok(()) => respond(stream, "200 OK", "application/json", b"{}"),
        Err(error) => {
            let status = match error {
                StartError::Invalid(_) => "400 Bad Request",
                StartError::Busy(_) => "409 Conflict",
                StartError::Failed(_) => "500 Internal Server Error",
            };
            respond(
                stream,
                status,
                "application/json",
                serde_json::json!({ "error": error.message() })
                    .to_string()
                    .as_bytes(),
            )
        }
    }
}

#[derive(serde::Deserialize)]
struct QueueRequest {
    action: String,
    repo: String,
    pid: i32,
}

/// Moves a waiter or takes over for it, as the commands do, then samples so
/// the pages see the new queue.
fn queue_action(model: &Model, body: &[u8]) -> crate::fail::Result<()> {
    let request: QueueRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => crate::fail!("the request: {error}"),
    };
    match request.action.as_str() {
        "top" => crate::queue::top(&model.places, &request.repo, request.pid)?,
        "takeover" => {
            crate::queue::takeover(&model.places, &request.repo, Some(request.pid), None)?
        }
        other => crate::fail!("no action {other:?}"),
    }
    model.sample_now().map(|_| ())
}

fn json(stream: &mut TcpStream, body: crate::fail::Result<Vec<u8>>) -> std::io::Result<()> {
    match body {
        Ok(body) => respond(stream, "200 OK", "application/json", &body),
        Err(crate::fail::Fail::Message(message)) => respond(
            stream,
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({ "error": message })
                .to_string()
                .as_bytes(),
        ),
        Err(crate::fail::Fail::Exit(code)) => respond(
            stream,
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({ "error": format!("exit {code}") })
                .to_string()
                .as_bytes(),
        ),
    }
}

/// Streams `state` events until the page goes. The page is watched, so the
/// processes are read, while the stream lasts.
fn events(mut stream: TcpStream, model: &Model) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n"
    )?;
    model.watch(true);
    let sent = (|| -> std::io::Result<()> {
        let mut seen = 0;
        // The lists last sent: another `Arc` means they changed.
        let mut sent_lists: Option<Arc<String>> = None;
        let mut sent_hooks: Option<Arc<String>> = None;
        loop {
            let started = Instant::now();
            let page = match model.page() {
                Ok(page) => page,
                Err(error) => {
                    let message = match error {
                        crate::fail::Fail::Message(message) => message,
                        crate::fail::Fail::Exit(code) => format!("exit {code}"),
                    };
                    write!(
                        stream,
                        "event: failure\ndata: {}\n\n",
                        serde_json::json!({ "error": message })
                    )?;
                    stream.flush()?;
                    seen = model.wait_change(seen);
                    continue;
                }
            };
            seen = page.version;
            let data = serde_json::to_string(&page).expect("the state serializes");
            write!(stream, "event: state\ndata: {data}\n\n")?;
            match model.lists_json() {
                Ok(lists)
                    if !sent_lists
                        .as_ref()
                        .is_some_and(|sent| Arc::ptr_eq(sent, &lists)) =>
                {
                    write!(stream, "event: lists\ndata: {lists}\n\n")?;
                    sent_lists = Some(lists);
                }
                Ok(_) => {}
                Err(error) => {
                    write!(
                        stream,
                        "event: failure\ndata: {}\n\n",
                        serde_json::json!({ "error": error.text() })
                    )?;
                }
            }
            let hooks = model.hooks_json();
            if !sent_hooks
                .as_ref()
                .is_some_and(|sent| Arc::ptr_eq(sent, &hooks))
            {
                write!(stream, "event: hooks\ndata: {hooks}\n\n")?;
                sent_hooks = Some(hooks);
            }
            stream.flush()?;
            if let Some(left) = EVENT_GAP.checked_sub(started.elapsed()) {
                std::thread::sleep(left);
            }
            seen = model.wait_change(seen);
        }
    })();
    model.watch(false);
    sent
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
