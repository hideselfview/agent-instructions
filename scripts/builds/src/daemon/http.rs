//! The page's server: a few GET routes on 127.0.0.1, four POST routes, and one
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
//!     POST /api/question-effect  `{"project", "id", "effect"}`: the same as
//!                   `builds question edit <id> --effect <effect>` in that
//!                   project, and the daemon records the edit as the
//!                   person's; only from this Mac (effect.rs)
//!     POST /design/<project>/<design id>/approve  records the person's approval of that
//!                   design change and tells the session that recorded it;
//!                   only from this Mac (see `approve`)
//!     /api/lists    the lists JSON (lists/view.rs, plus each running item's
//!                   claim facts), as the `lists` event carries it
//!     /api/hooks    the hook runs' figures and latest runs (hook_stats.rs),
//!                   as the `hooks` event carries them
//!     /design/<project>/<design id>
//!                   the review page of an open design change, drawn now,
//!                   which links the scratch boards in the change's worktree
//!                   when it is asked for on this Mac
//!     /fonts/sf-mono/<file>
//!                   a file of the SF Mono family from Terminal.app, which the
//!                   boards' pages load (`boards::fonts`)
//!     /design/<project>/<design id>/board/<n>/now and /proposed
//!     /design/<project>/<design id>/changed/<k>/proposed
//!     /design/<project>/<design id>/deleted/<k>/now
//!                   a board of that page as it stands on origin/main (now)
//!                   or at the change's last commit (proposed), drawn now:
//!                   a board the page shows, a board the change changes that
//!                   the page lists by name, or a board the change deletes;
//!                   the page's frames load these
//!     /design/<project>/scratch/<worktree>/<board>, with /events and
//!     /<theme>/now and /<theme>/worktree under it
//!                   the scratch view of a board, drawn from a worktree's
//!                   files as they are, and the pages and events it reads;
//!                   only on this Mac (scratch_view.rs)
//!     /designs/<project>
//!                   the Designs page of the project: every board of the
//!                   repo that holds its design files as it stands on
//!                   origin/main, its design system, and, when it is asked
//!                   for on this Mac, the scratch boards of the repo's
//!                   worktrees (designs.rs)
//!     /designs/<project>/publish/events
//!                   `publish` events: the project's publishing (its newest
//!                   publish request and the newest that published,
//!                   lists/publish.rs) on connect
//!                   and each time a read of the stores finds them changed;
//!                   the Designs page's form follows these
//!     POST /designs/<project>/publish  `{"commit"}`: records a request to
//!                   publish the designs at that commit and wakes the design
//!                   session listening for the project; only from this Mac
//!     /designs/<project>/<commit>/board/<name>
//!     /designs/<project>/<commit>/component/<name>
//!     /designs/<project>/<commit>/colors/<n>
//!     /designs/<project>/<commit>/colors/comparison
//!                   a board, a component's preview, the nth group of the
//!                   colors or the colors' comparison, at that commit, drawn
//!                   now; the Designs page's frames load these
//!     /designs/<project>/component/<name>
//!                   the page of a component's shots at the commit
//!                   origin/main names (component_shots.rs)
//!     /designs/<project>/<commit>/shots/<name>/events
//!                   `shots` events: what the component page shows of the
//!                   component at that commit, on connect and each time it
//!                   changes
//!     POST /designs/<project>/<commit>/shots/<name>/capture  takes the
//!                   component's shots of that commit on each lane where
//!                   builds has none cached; only from this Mac
//!     /designs/<project>/<commit>/shot/<lane>/<shot>/board.png and /shot.png
//!                   a pair's pictures in the cached run of the commit
//!     /events       `state` events give the whole state, at most four times
//!                   a second. `lists` and `hooks` events give those readings
//!                   on connect and when they change, including when the
//!                   checkout reading fails.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::component_shots::Side as PictureSide;
use super::model::Model;
use crate::design_render::Side;
use crate::lane::Lane;

/// The page, with the colors and fonts it shares with the Designs page.
static PAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    crate::design_render::fill(
        include_str!("page.html"),
        &[
            ("PALETTE", super::designs::PALETTE),
            ("CONTROLS", super::designs::CONTROLS),
            ("CONTROLS_JS", super::designs::CONTROLS_JS),
        ],
    )
});
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

pub(super) fn respond(
    stream: &mut TcpStream,
    status: &str,
    kind: &str,
    body: &[u8],
) -> std::io::Result<()> {
    respond_with(stream, status, kind, &[], body)
}

/// `respond`, with the header lines <headers> besides the usual ones.
fn respond_with(
    stream: &mut TcpStream,
    status: &str,
    kind: &str,
    headers: &[&str],
    body: &[u8],
) -> std::io::Result<()> {
    let extra: String = headers.iter().map(|line| format!("{line}\r\n")).collect();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{extra}Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

/// The SF Mono file <name> of the family's folder (`Places::fonts`), for a
/// board's page to load.
fn serve_font(stream: &mut TcpStream, model: &Model, name: &str) -> std::io::Result<()> {
    match std::fs::read(model.places.fonts.join(name)) {
        // A board's frame runs in a sandbox with no origin of its own, so it
        // loads the font as a cross-origin request.
        Ok(font) => respond_with(
            stream,
            "200 OK",
            "font/otf",
            &["Access-Control-Allow-Origin: *"],
            &font,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => respond(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"no such SF Mono file on this Mac\n",
        ),
        Err(error) => {
            let message = format!("cannot read the SF Mono file {name}: {error}");
            crate::text::note(&message);
            respond(
                stream,
                "500 Internal Server Error",
                "text/plain; charset=utf-8",
                format!("{message}\n").as_bytes(),
            )
        }
    }
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
    let asker = Asker {
        here: stream.peer_addr().is_ok_and(|peer| peer.ip().is_loopback()),
        host: host.as_deref(),
        origin: origin.as_deref(),
        names,
    };
    if !allowed(method, &asker) {
        return refuse_not_here(&mut stream);
    }
    if method == "POST"
        && (path == "/api/queue" || path == "/api/build-app" || path == "/api/question-effect")
    {
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
        if path == "/api/question-effect" {
            return json(
                &mut stream,
                question_effect(model, &body).map(|()| b"{}".to_vec()),
            );
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
    if method == "GET"
        && let Some(project) = path.strip_suffix("/events").and_then(publish_target)
    {
        return publish_events(stream, model, project);
    }
    if let Some((project, commit, component, ask)) = shots_target(path) {
        return match (method, ask) {
            ("GET", ShotsAsk::Events) => shots_events(stream, model, project, commit, component),
            ("POST", ShotsAsk::Capture) => {
                match model.capture_component(project, commit, component) {
                    Some(Ok(())) => json(&mut stream, Ok(b"{}".to_vec())),
                    Some(Err(error)) => {
                        let message = error.text();
                        crate::text::note(&message);
                        json_error(&mut stream, &message)
                    }
                    None => json_error(&mut stream, &no_design_files(project)),
                }
            }
            _ => respond(
                &mut stream,
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                b"GET the events, POST the capture\n",
            ),
        };
    }
    if let Some(project) = publish_target(path) {
        return match method {
            "POST" if length > MAX_BODY => respond(
                &mut stream,
                "413 Payload Too Large",
                "text/plain; charset=utf-8",
                b"too large\n",
            ),
            "POST" => ask_publish(&mut stream, model, project, &body),
            _ => respond(
                &mut stream,
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                b"POST only\n",
            ),
        };
    }
    if method != "GET" {
        return respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"GET only, and POST /api/queue, /api/build-app, /api/question-effect, /design/PROJECT/N/approve, /designs/PROJECT/publish and /designs/PROJECT/COMMIT/shots/COMPONENT/capture\n",
        );
    }
    if let Some(route) = super::scratch_view::route(path) {
        if !on_this_mac(&asker) {
            return refuse_not_here(&mut stream);
        }
        return super::scratch_view::serve(stream, model, route);
    }
    let here = on_this_mac(&asker);
    if let Some(boards::routes::Route::Font(name)) = boards::routes::route(path) {
        return serve_font(&mut stream, model, &name);
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
        _ if path.starts_with("/designs/") => {
            let route = designs_route(&path["/designs/".len()..]);
            if let Some(DesignsRoute::Picture(project, commit, lane, shot, side)) = route {
                return match model.shot_picture(project, commit, lane, shot, side) {
                    Some(Ok(Some(picture))) => {
                        respond(&mut stream, "200 OK", "image/png", &picture)
                    }
                    Some(Err(error)) => {
                        let message = error.text();
                        crate::text::note(&message);
                        respond(
                            &mut stream,
                            "500 Internal Server Error",
                            "text/plain; charset=utf-8",
                            format!("{message}\n").as_bytes(),
                        )
                    }
                    None | Some(Ok(None)) => respond(
                        &mut stream,
                        "404 Not Found",
                        "text/plain; charset=utf-8",
                        b"builds has no cached picture of that commit\n",
                    ),
                };
            }
            let drawn = route.and_then(|route| match route {
                DesignsRoute::Page(project) => model.designs_page(project, here),
                DesignsRoute::Board(project, commit, name) => {
                    model.designs_board(project, commit, name)
                }
                DesignsRoute::Component(project, commit, name) => {
                    model.designs_preview(project, commit, name)
                }
                DesignsRoute::Colors(project, commit, n) => {
                    model.designs_colors(project, commit, n)
                }
                DesignsRoute::ColorsComparison(project, commit) => {
                    model.designs_colors_comparison(project, commit)
                }
                DesignsRoute::ComponentShots(project, name) => {
                    model.component_shots_page(project, name, here)
                }
                DesignsRoute::Picture(..) => unreachable!("answered above"),
            });
            drawn_page(
                &mut stream,
                drawn,
                b"no such project with design files, or no such page of its designs\n",
            )
        }
        _ if path.starts_with("/design/") => {
            let drawn = design_route(&path["/design/".len()..]).and_then(|route| match route {
                DesignRoute::Page(project, id) => model.design_page(project, id, here),
                DesignRoute::Board(project, id, n, side) => {
                    model.design_board(project, id, n, side)
                }
                DesignRoute::Changed(project, id, k) => model.design_changed_board(project, id, k),
                DesignRoute::Deleted(project, id, k) => model.design_deleted_board(project, id, k),
            });
            drawn_page(&mut stream, drawn, b"no such open design change or board\n")
        }
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found\n",
        ),
    }
}

/// Answers with <drawn>, a page drawn now: the page, the error that stopped
/// it, or <missing> when the address names nothing.
pub(super) fn drawn_page(
    stream: &mut TcpStream,
    drawn: Option<crate::fail::Result<String>>,
    missing: &[u8],
) -> std::io::Result<()> {
    match drawn {
        Some(Ok(page)) => respond(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            page.as_bytes(),
        ),
        Some(Err(error)) => respond(
            stream,
            "500 Internal Server Error",
            "text/plain; charset=utf-8",
            format!("{}\n", error.text()).as_bytes(),
        ),
        None => respond(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            missing,
        ),
    }
}

/// What a `/designs/...` path asks for.
#[derive(Debug, PartialEq, Eq)]
enum DesignsRoute<'a> {
    /// The Designs page of the project.
    Page(&'a str),
    /// Board <name> of the project's design files at the commit.
    Board(&'a str, &'a str, &'a str),
    /// Component <name>'s preview at the commit.
    Component(&'a str, &'a str, &'a str),
    /// Group <n> of the colors at the commit.
    Colors(&'a str, &'a str, usize),
    /// The colors' comparison at the commit.
    ColorsComparison(&'a str, &'a str),
    /// The page of component <name>'s shots.
    ComponentShots(&'a str, &'a str),
    /// A picture of the pair <shot> on the lane, in the cached run of
    /// the commit.
    Picture(&'a str, &'a str, Lane, &'a str, PictureSide),
}

/// <rest> is the path after `/designs/`: `<project>`,
/// `<project>/component/<name>`, `<project>/<commit>/board/<name>`,
/// `<project>/<commit>/component/<name>`, `<project>/<commit>/colors/<n>`,
/// `<project>/<commit>/colors/comparison` or
/// `<project>/<commit>/shot/<lane>/<shot>/board.png` (or `shot.png`), where
/// the commit is a full commit id, as the page names it.
fn designs_route(rest: &str) -> Option<DesignsRoute<'_>> {
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [project] if !project.is_empty() => Some(DesignsRoute::Page(project)),
        [project, "component", name] if !name.is_empty() => {
            Some(DesignsRoute::ComponentShots(project, name))
        }
        [project, commit, "shot", lane, shot, file] if super::designs::is_commit(commit) => {
            Some(DesignsRoute::Picture(
                project,
                commit,
                Lane::parse(lane)?,
                shot,
                PictureSide::parse(file.strip_suffix(".png")?)?,
            ))
        }
        [project, commit, "board", name] if super::designs::is_commit(commit) => {
            Some(DesignsRoute::Board(project, commit, name))
        }
        [project, commit, "component", name] if super::designs::is_commit(commit) => {
            Some(DesignsRoute::Component(project, commit, name))
        }
        [project, commit, "colors", "comparison"] if super::designs::is_commit(commit) => {
            Some(DesignsRoute::ColorsComparison(project, commit))
        }
        [project, commit, "colors", n] if super::designs::is_commit(commit) => {
            Some(DesignsRoute::Colors(project, commit, n.parse().ok()?))
        }
        _ => None,
    }
}

/// What a `/design/...` path asks for.
#[derive(Debug, PartialEq, Eq)]
enum DesignRoute<'a> {
    /// The review page of design change <id> of the project.
    Page(&'a str, u32),
    /// The now or proposed drawing of the <n>th board of its review page.
    Board(&'a str, u32, usize, Side),
    /// The proposal of the <k>th board place of the change that its review
    /// page does not show, which the page lists by name.
    Changed(&'a str, u32, usize),
    /// The now of the <k>th board it deletes.
    Deleted(&'a str, u32, usize),
}

/// <rest> is the path after `/design/`: `<project>/<id>`,
/// `<project>/<id>/board/<n>/now`, `.../board/<n>/proposed`,
/// `<project>/<id>/changed/<k>/proposed` or `<project>/<id>/deleted/<k>/now`. Ids count per project, so the
/// project is in every address.
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
        [project, id, "changed", k, "proposed"] => Some(DesignRoute::Changed(
            project,
            id.parse().ok()?,
            k.parse().ok()?,
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

/// Is <asker> on this Mac: a loopback peer address that asks for a page at a
/// loopback name? The scratch view (scratch_view.rs), which shows files that
/// no commit holds, answers only such a request, and only such a request
/// gets the scratch boards on the Designs page and a review page.
fn on_this_mac(asker: &Asker<'_>) -> bool {
    asker.here && asker.host.is_some_and(is_this_mac)
}

/// Does <asker> come from this Mac (`on_this_mac`), with the page served at
/// a loopback name as its Origin too (one of the names the daemon answers
/// to)? Every route that changes state requires it, since the Host and
/// Origin headers alone can be forged by anyone who can reach the dashboard
/// when it listens on the Wi-Fi's address.
fn from_this_mac(asker: &Asker<'_>) -> bool {
    on_this_mac(asker)
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

/// What a `/designs/<project>/<commit>/shots/<name>/...` path asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShotsAsk {
    /// The stream of what the component page shows.
    Events,
    /// Taking the component's shots where builds has none cached.
    Capture,
}

/// The project, commit, component and ask of
/// `/designs/<project>/<commit>/shots/<name>/events` or `.../capture`.
fn shots_target(path: &str) -> Option<(&str, &str, &str, ShotsAsk)> {
    let parts: Vec<&str> = path.strip_prefix("/designs/")?.split('/').collect();
    match parts.as_slice() {
        [project, commit, "shots", name, ask] if super::designs::is_commit(commit) => {
            let ask = match *ask {
                "events" => ShotsAsk::Events,
                "capture" => ShotsAsk::Capture,
                _ => return None,
            };
            Some((project, commit, name, ask))
        }
        _ => None,
    }
}

/// Streams `shots` events of component <component> of project <project> at
/// <commit> until the page goes: what the component page shows
/// (`component_shots::shots`) on connect and each time it differs from what
/// the stream sent last, read again on each change of the dashboard's state,
/// which a shots run's start, output and end move.
fn shots_events(
    mut stream: TcpStream,
    model: &Model,
    project: &str,
    commit: &str,
    component: &str,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n"
    )?;
    let mut seen = 0;
    let mut sent: Option<String> = None;
    loop {
        let (event, error) = match model.component_shots(project, commit, component) {
            Some(Ok(shots)) => (
                serde_json::to_string(&shots).expect("the shots serialize"),
                None,
            ),
            Some(Err(error)) => {
                let message = error.text();
                (json_error_body(&message), Some(message))
            }
            None => (json_error_body(&no_design_files(project)), None),
        };
        if sent.as_ref() != Some(&event) {
            if let Some(error) = error {
                crate::text::note(error);
            }
            write!(stream, "event: shots\ndata: {event}\n\n")?;
            stream.flush()?;
            sent = Some(event);
        }
        seen = model.wait_change(seen);
    }
}

/// The project of `/designs/<project>/publish`.
fn publish_target(path: &str) -> Option<&str> {
    path.strip_prefix("/designs/")?
        .strip_suffix("/publish")
        .filter(|project| !project.is_empty() && !project.contains('/'))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishRequest {
    /// The commit or branch to publish, such as origin/main.
    commit: String,
}

/// Asks the design session listening for <project> to publish its designs at
/// the commit <body> names (`POST /designs/PROJECT/publish`). Only the person
/// at this Mac asks (`handle` checks `from_this_mac`). Answers the request;
/// 404 for a project with no design files, 409 when publishing is refused,
/// such as when no design session listens, with the error.
fn ask_publish(
    stream: &mut TcpStream,
    model: &Model,
    project: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let request: PublishRequest = match serde_json::from_slice(body) {
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
    if request.commit.trim().is_empty() || request.commit.starts_with('-') {
        let message =
            serde_json::json!({ "error": "name a commit or branch to publish" }).to_string();
        return respond(
            stream,
            "400 Bad Request",
            "application/json",
            message.as_bytes(),
        );
    }
    match model.ask_publish(project, request.commit.trim()) {
        None => respond(
            stream,
            "404 Not Found",
            "application/json",
            json_error_body(&no_design_files(project)).as_bytes(),
        ),
        Some(Ok(asked)) => respond(
            stream,
            "200 OK",
            "application/json",
            &serde_json::to_vec(&asked).expect("the request serializes"),
        ),
        Some(Err(error)) => respond(
            stream,
            "409 Conflict",
            "application/json",
            serde_json::json!({ "error": error.text() })
                .to_string()
                .as_bytes(),
        ),
    }
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
/// "session": null|"unrecorded"|"told"|"queued"|"ended"}`.
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

/// Moves a waiter or takes over for it, as the commands do, then reads the
/// processes so the pages see the new queue.
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
    model.refresh_processes()
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectRequest {
    project: String,
    id: u32,
    effect: String,
}

/// Gives a question the effect the person chose on the page (effect.rs).
fn question_effect(model: &Model, body: &[u8]) -> crate::fail::Result<()> {
    let request: EffectRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => crate::fail!("the request: {error}"),
    };
    super::effect::set_question_effect(
        &model.places,
        super::model::now_seconds(),
        &request.project,
        request.id,
        &request.effect,
    )
}

fn json(stream: &mut TcpStream, body: crate::fail::Result<Vec<u8>>) -> std::io::Result<()> {
    match body {
        Ok(body) => respond(stream, "200 OK", "application/json", &body),
        Err(error) => json_error(stream, &error.text()),
    }
}

fn json_error(stream: &mut TcpStream, message: &str) -> std::io::Result<()> {
    respond(
        stream,
        "500 Internal Server Error",
        "application/json",
        json_error_body(message).as_bytes(),
    )
}

fn no_design_files(project: &str) -> String {
    format!("project {project} has no design files")
}

fn json_error_body(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

/// Sends the dashboard's state, lists and hook figures until the page goes.
/// The daemon samples processes more often while the page watches.
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
            match model.page() {
                Ok(page) => {
                    seen = page.version;
                    let data = serde_json::to_string(&page).expect("the state serializes");
                    write!(stream, "event: state\ndata: {data}\n\n")?;
                }
                Err(error) => {
                    write!(
                        stream,
                        "event: failure\ndata: {}\n\n",
                        json_error_body(&error.text())
                    )?;
                }
            }
            match model.lists_json() {
                Ok(lists) => send_if_changed(&mut stream, "lists", lists, &mut sent_lists)?,
                Err(error) => {
                    write!(
                        stream,
                        "event: failure\ndata: {}\n\n",
                        json_error_body(&error.text())
                    )?;
                }
            }
            send_if_changed(&mut stream, "hooks", model.hooks_json(), &mut sent_hooks)?;
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

/// Streams `publish` events of project <project> until the page goes: its
/// publishing (`designs::standing`) on connect and after each read of the
/// stores that found them changed, which every write to a store causes, or a
/// `failure` event when the store can't be read.
fn publish_events(mut stream: TcpStream, model: &Model, project: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n"
    )?;
    let mut seen = 0;
    let mut sent: Option<u64> = None;
    loop {
        let read = model.lists_version();
        if sent != Some(read) {
            match super::designs::standing(&model.places, project) {
                Ok(standing) => write!(
                    stream,
                    "event: publish\ndata: {}\n\n",
                    serde_json::to_string(&standing).expect("the standing serializes")
                )?,
                Err(error) => write!(
                    stream,
                    "event: failure\ndata: {}\n\n",
                    serde_json::json!({ "error": error.text() })
                )?,
            }
            stream.flush()?;
            sent = Some(read);
        }
        seen = model.wait_change(seen);
    }
}

/// Writes <data> to <stream> as event <name> unless <sent> holds the same
/// `Arc`, the one this stream last sent, and keeps <data> in <sent>.
fn send_if_changed(
    stream: &mut TcpStream,
    name: &str,
    data: Arc<String>,
    sent: &mut Option<Arc<String>>,
) -> std::io::Result<()> {
    if sent.as_ref().is_some_and(|last| Arc::ptr_eq(last, &data)) {
        return Ok(());
    }
    write!(stream, "event: {name}\ndata: {data}\n\n")?;
    *sent = Some(data);
    Ok(())
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
