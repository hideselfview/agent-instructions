//! The page's server: a few GET routes on 127.0.0.1, and one stream of
//! server-sent events per open page. A request whose Host isn't this
//! server's own address is refused, so a page from elsewhere that resolves
//! its name to 127.0.0.1 can't read these.
//!
//!     /             the page (page.html, built into the binary)
//!     /api/state    the whole state as JSON
//!     /api/health   health, judged now
//!     /events       `state` events: the whole state on connect and each time
//!                   it changes, at most four a second

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::model::Model;

const PAGE: &str = include_str!("page.html");
/// The least time between two events on one stream: pacing for the page.
const EVENT_GAP: Duration = Duration::from_millis(250);

pub fn serve(listener: TcpListener, port: u16, model: Arc<Model>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let model = model.clone();
        std::thread::spawn(move || {
            if let Err(error) = handle(stream, port, &model)
                && !matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset)
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
    let mut host = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("host")
        {
            host = Some(value.trim().to_string());
        }
    }
    let mut parts = request.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let allowed = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !host.is_some_and(|host| allowed.contains(&host)) {
        return respond(&mut stream, "403 Forbidden", "text/plain; charset=utf-8", b"only for this machine's own browser\n");
    }
    if method != "GET" {
        return respond(&mut stream, "405 Method Not Allowed", "text/plain; charset=utf-8", b"GET only\n");
    }
    let path = target.split('?').next().unwrap_or("");
    match path {
        "/" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        "/api/state" => json(&mut stream, model.page().map(|page| serde_json::to_vec(&page).expect("the state serializes"))),
        "/api/health" => json(&mut stream, model.sample_now().map(|health| serde_json::to_vec(&health).expect("health serializes"))),
        "/events" => events(stream, model),
        _ => respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"not found\n"),
    }
}

fn json(stream: &mut TcpStream, body: crate::fail::Result<Vec<u8>>) -> std::io::Result<()> {
    match body {
        Ok(body) => respond(stream, "200 OK", "application/json", &body),
        Err(crate::fail::Fail::Message(message)) => respond(
            stream,
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({ "error": message }).to_string().as_bytes(),
        ),
        Err(crate::fail::Fail::Exit(code)) => respond(
            stream,
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({ "error": format!("exit {code}") }).to_string().as_bytes(),
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
        loop {
            let started = Instant::now();
            let page = match model.page() {
                Ok(page) => page,
                Err(error) => {
                    let message = match error {
                        crate::fail::Fail::Message(message) => message,
                        crate::fail::Fail::Exit(code) => format!("exit {code}"),
                    };
                    write!(stream, "event: failure\ndata: {}\n\n", serde_json::json!({ "error": message }))?;
                    stream.flush()?;
                    seen = model.wait_change(seen);
                    continue;
                }
            };
            seen = page.version;
            let data = serde_json::to_string(&page).expect("the state serializes");
            write!(stream, "event: state\ndata: {data}\n\n")?;
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
