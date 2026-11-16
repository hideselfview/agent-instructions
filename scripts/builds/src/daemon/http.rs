//! The page's server: a few GET routes on 127.0.0.1, three POST routes, and one
//! stream of server-sent events per open page. A request whose Host isn't
//! this server's own address is refused, so a page from elsewhere that
//! resolves its name to 127.0.0.1 can't read these; a POST whose Origin isn't
//! the page's own is refused too, so a web page can't trigger it. (A browser
//! sends the real Origin on every cross-site POST and a page can't change it,
//! so no token is needed; a POST with no Origin, which a browser never sends,
//! is refused as well.) Headers can be forged by anyone who can reach the
//! server, so every POST also requires a loopback peer address and a loopback
//! Host and Origin (`from_this_mac`): the dashboard may listen on the Wi-Fi's
//! address, where the page is only to look at.
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
//!     POST /design/<project>/<design id>/approve  records the person's approval of that
//!                   design change and tells the session that recorded it;
//!                   only from this Mac (see `approve`)
//!     /api/lists    the lists JSON (lists/view.rs, plus each running item's
//!                   claim facts), as the `lists` event carries it
//!     /api/hooks    the hook runs' figures and latest runs (hook_stats.rs),
//!                   as the `hooks` event carries them
//!     /design/<project>/<design id>
//!                   the review page of an open design change, drawn now
//!     /design/<project>/<design id>/board/<n>/now and /proposed
//!     /design/<project>/<design id>/deleted/<k>/now
//!                   a board of that page as it stands on origin/main (now)
//!                   or at the change's last commit (proposed), drawn now;
//!                   the page's frames load these
//!     /events       `state` events: the whole state on connect and each time
//!                   it changes, at most four a second; `lists` events: the
//!                   lists JSON on connect and each time it changes, never
//!                   with a `state` event that only repeats the processes

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::model::Model;
use crate::design_render::Side;

const PAGE: &str = include_str!("page.html");
/// The most a POST's body may hold.
const MAX_BODY: usize = 4096;
/// The least time between two events on one stream: pacing for the page.
const EVENT_GAP: Duration = Duration::from_millis(250);

/// Answers the page's requests on <listener>. <names> are the `host:port`
/// names a request may give in its Host header, and the page's own origins:
/// this machine's names, and any address the daemon also listens on.
pub fn serve(listener: TcpListener, names: Arc<[String]>, model: Arc<Model>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let model = model.clone();
        let names = names.clone();
        std::thread::spawn(move || {
            if let Err(error) = handle(stream, &names, &model)
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

fn handle(mut stream: TcpStream, names: &[String], model: &Model) -> std::io::Result<()> {
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
    if !host
        .as_deref()
        .is_some_and(|host| names.iter().any(|name| name == host))
    {
        return respond(
            &mut stream,
            "403 Forbidden",
            "text/plain; charset=utf-8",
            b"only at the addresses the dashboard listens on\n",
        );
    }
    let path = target.split('?').next().unwrap_or("");
    if !allowed(
        method,
        &Asker {
            here: stream.peer_addr().is_ok_and(|peer| peer.ip().is_loopback()),
            host: host.as_deref(),
            origin: origin.as_deref(),
            names,
        },
    ) {
        return refuse_not_here(&mut stream);
    }
    if method == "POST" && (path == "/api/queue" || path == "/api/build-app") {
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
    if method == "POST"
        && let Some((project, id)) = design_approve_target(path)
    {
        return approve(&mut stream, model, project, id);
    }
    if method != "GET" {
        return respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"GET only, and POST /api/queue, /api/build-app and /design/PROJECT/N/approve\n",
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
            match model.health_now() {
                Ok(Some(health)) => Ok(serde_json::to_vec(&health).expect("health serializes")),
                Ok(None) => Err(crate::fail::Fail::Message(
                    "the daemon has not read the processes yet".into(),
                )),
                Err(error) => Err(error),
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
        "/events" => events(stream, model),
        _ if path.starts_with("/design/") => {
            let drawn = design_route(&path["/design/".len()..]).and_then(|route| match route {
                DesignRoute::Page(project, id) => model.design_page(project, id),
                DesignRoute::Board(project, id, n, side) => {
                    model.design_board(project, id, n, side)
                }
                DesignRoute::Deleted(project, id, k) => model.design_deleted_board(project, id, k),
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

/// What a `/design/...` path asks for.
#[derive(Debug, PartialEq, Eq)]
enum DesignRoute<'a> {
    /// The review page of design change <id> of the project.
    Page(&'a str, u32),
    /// The now or proposed drawing of the <n>th board of its review page.
    Board(&'a str, u32, usize, Side),
    /// The now of the <k>th board it deletes.
    Deleted(&'a str, u32, usize),
}

/// <rest> is the path after `/design/`: `<project>/<id>`,
/// `<project>/<id>/board/<n>/now`, `.../board/<n>/proposed` or
/// `<project>/<id>/deleted/<k>/now`. Ids count per project, so the project
/// is in every address.
fn design_route(rest: &str) -> Option<DesignRoute<'_>> {
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [project, id] => Some(DesignRoute::Page(project, id.parse().ok()?)),
        [project, id, "board", n, "now"] => Some(DesignRoute::Board(
            project,
            id.parse().ok()?,
            n.parse().ok()?,
            Side::Now,
        )),
        [project, id, "board", n, "proposed"] => Some(DesignRoute::Board(
            project,
            id.parse().ok()?,
            n.parse().ok()?,
            Side::Proposed,
        )),
        [project, id, "deleted", k, "now"] => Some(DesignRoute::Deleted(
            project,
            id.parse().ok()?,
            k.parse().ok()?,
        )),
        _ => None,
    }
}

/// Does the Origin header name this server's own page, at one of <names>?
fn from_own_page(origin: Option<&str>, names: &[String]) -> bool {
    origin.is_some_and(|origin| {
        origin
            .strip_prefix("http://")
            .is_some_and(|name| names.iter().any(|own| own == name))
    })
}

/// Who is asking for a change of state.
struct Asker<'a> {
    /// The connection came from a loopback address, which no request from
    /// another machine does whatever headers it sends.
    here: bool,
    host: Option<&'a str>,
    origin: Option<&'a str>,
    names: &'a [String],
}

/// Is <name> (`host:port`) this Mac, as 127.0.0.1, localhost or [::1]?
fn is_this_mac(name: &str) -> bool {
    let host = name.rsplit_once(':').map_or(name, |(host, _)| host);
    host == "127.0.0.1" || host == "localhost" || host == "[::1]"
}

/// Does <asker> come from this Mac: a loopback peer address, and the page
/// served at a loopback name as both its Host and its Origin (one of the
/// names the daemon answers to)? Every route that changes state requires it,
/// since the Host and Origin headers alone can be forged by anyone who can
/// reach the dashboard when it listens on the Wi-Fi's address.
fn from_this_mac(asker: &Asker<'_>) -> bool {
    asker.here
        && asker.host.is_some_and(is_this_mac)
        && from_own_page(asker.origin, asker.names)
        && asker
            .origin
            .and_then(|origin| origin.strip_prefix("http://"))
            .is_some_and(is_this_mac)
}

/// Is a <method> request by <asker> allowed past the one gate before every
/// route: a request that changes state must come from this Mac.
fn allowed(method: &str, asker: &Asker<'_>) -> bool {
    !changes_state(method) || from_this_mac(asker)
}

/// Does a request change state? Every method but GET and HEAD may, so a route
/// added later is covered: `handle` requires `from_this_mac` of each before it
/// reaches any route.
fn changes_state(method: &str) -> bool {
    method != "GET" && method != "HEAD"
}

/// The project and design change id of `/design/<project>/<id>/approve`.
fn design_approve_target(path: &str) -> Option<(&str, u32)> {
    let (project, id) = path
        .strip_prefix("/design/")?
        .strip_suffix("/approve")?
        .split_once('/')?;
    Some((project, id.parse().ok()?))
}

fn refuse_not_here(stream: &mut TcpStream) -> std::io::Result<()> {
    respond(
        stream,
        "403 Forbidden",
        "application/json",
        serde_json::json!({ "error": "only on this Mac, from the dashboard at 127.0.0.1 or localhost" })
            .to_string()
            .as_bytes(),
    )
}

/// Records the person's approval of design change <id> and tells the session
/// that recorded it (`POST /design/PROJECT/N/approve`). Only the person at this Mac
/// approves (`handle` checks `from_this_mac`). Answers `{"approved": true, "already": bool,
/// "session": null|"unrecorded"|"told"|"ended"}`.
fn approve(stream: &mut TcpStream, model: &Model, project: &str, id: u32) -> std::io::Result<()> {
    use crate::lists::design_session::Approval;
    let answer =
        super::approve::approve_design(&model.places, super::model::now_seconds(), project, id)
            .map(|approval| {
                let (already, session) = match approval {
                    Approval::Already => (true, None),
                    Approval::Recorded(session) => (false, Some(session)),
                };
                serde_json::json!({ "approved": true, "already": already, "session": session })
                    .to_string()
                    .into_bytes()
            });
    json(stream, answer)
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
