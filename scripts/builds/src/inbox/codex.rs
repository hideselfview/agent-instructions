//! Codex keeps a thread's queue while it is idle. Sending uses the CLI's
//! local queue and requests a resume. A judge can receive work as long as its
//! saved thread exists, even if no terminal has it open.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use serde_json::{Value, json};

use crate::fail;
use crate::fail::{Context, Result};
use crate::inbox::Sent;

use crate::ack::agent::ThreadId;

const WAIT: Duration = Duration::from_secs(10);

pub(super) fn queue(thread: &ThreadId, program: &std::path::Path, text: &str) -> Result<Sent> {
    let mut command = Command::new(program);
    command.args([
        "queue",
        "--thread",
        thread.id(),
        "--message",
        text,
        "--resume",
    ]);
    let deadline = Instant::now() + WAIT;
    let (mut output, mut child) = start(command, Mode::Queue, deadline)?;
    let mut response = String::new();
    let read = output
        .read_to_string(&mut response)
        .context(|| format!("reading the Codex queue response for {}", thread.id()));
    let status = match read {
        Ok(_) => finish(&mut child, deadline),
        Err(error) => crate::fail::with_cleanup(Err(error), end(child)),
    }?;
    if status.success() {
        let mut resumed = true;
        for line in response.lines() {
            let warning = line.to_ascii_lowercase();
            if warning.starts_with("warning:") {
                crate::output::note(line);
                resumed &= !warning.contains("resume");
            }
        }
        return Ok(if resumed {
            Sent::Delivered
        } else {
            Sent::Queued
        });
    }
    if response.contains(&format!("no rollout found for thread id {}", thread.id())) {
        return Ok(Sent::Gone);
    }
    fail!(
        "Codex could not queue a message for {} ({status}): {}",
        thread.id(),
        response.trim()
    );
}

pub(super) fn exists(thread: &ThreadId, program: &std::path::Path) -> Result<bool> {
    let mut command = Command::new(program);
    command.args(["app-server", "--listen", "stdio://"]);
    let deadline = Instant::now() + WAIT;
    let (errors, error_writer) = output_stream(deadline)?;
    let (output, mut child) = start(command, Mode::Rpc(error_writer), deadline)?;
    std::thread::scope(|scope| {
        let diagnostic = std::thread::Builder::new().spawn_scoped(scope, || {
            let mut errors = errors;
            let mut diagnostic = String::new();
            errors
                .read_to_string(&mut diagnostic)
                .context(|| "reading Codex app-server diagnostics".to_string())?;
            Ok(diagnostic)
        });
        let diagnostic =
            match diagnostic.context(|| "starting the Codex diagnostics reader".to_string()) {
                Ok(diagnostic) => diagnostic,
                Err(error) => return crate::fail::with_cleanup(Err(error), end(child)),
            };
        let result = (|| {
            let stdin = child
                .stdin
                .as_mut()
                .expect("Codex app-server stdin is piped");
            let mut output = BufReader::new(output);
            let initialized = request(
                stdin,
                &mut output,
                1,
                "initialize",
                json!({"clientInfo": {"name": "builds", "version": env!("CARGO_PKG_VERSION")}}),
            )?;
            if initialized.get("result").is_none() {
                fail!("Codex could not initialize the thread reader: {initialized}");
            }
            let read = request(
                stdin,
                &mut output,
                2,
                "thread/read",
                json!({"threadId": thread.id(), "includeTurns": false}),
            )?;
            if let Some(id) = read["result"]["thread"]["id"].as_str() {
                if id != thread.id() {
                    fail!(
                        "Codex returned thread {id} when builds asked for {}.",
                        thread.id()
                    );
                }
                Ok(true)
            } else if read["error"]["code"].as_i64() == Some(-32600)
                && read["error"]["message"].as_str()
                    == Some(&format!("thread not loaded: {}", thread.id()))
            {
                Ok(false)
            } else {
                fail!("Codex could not read thread {}: {}", thread.id(), read);
            }
        })();
        let result = crate::fail::with_cleanup(result, end(child));
        let diagnostic: Result<String> = diagnostic
            .join()
            .expect("the diagnostics reader does not panic");
        match (result, diagnostic) {
            (Err(error), Ok(detail)) if !detail.trim().is_empty() => {
                fail!("{}. Codex reports: {}", error.text(), detail.trim())
            }
            (result, diagnostic) => crate::fail::with_cleanup(result, diagnostic.map(drop)),
        }
    })
}

/// Bounds all reads of one exchange, including reads that return data without
/// completing a response.
struct DeadlineReader {
    stream: UnixStream,
    deadline: Instant,
}

impl Read for DeadlineReader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        let millis = remaining.as_millis().max(1);
        let timeout =
            libc::c_int::try_from(millis).expect("the Codex deadline fits in milliseconds");
        let mut ready = libc::pollfd {
            fd: self.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // The socket has one reader. After poll reports data or a hangup,
        // reading it cannot wait for another writer.
        match unsafe { libc::poll(&mut ready, 1, timeout) } {
            0 => Err(std::io::ErrorKind::TimedOut.into()),
            -1 => Err(std::io::Error::last_os_error()),
            _ => self.stream.read(bytes),
        }
    }
}

fn output_stream(deadline: Instant) -> Result<(DeadlineReader, UnixStream)> {
    let (stream, writer) =
        UnixStream::pair().context(|| "opening the Codex output stream".to_string())?;
    Ok((DeadlineReader { stream, deadline }, writer))
}

enum Mode {
    Queue,
    Rpc(UnixStream),
}

fn start(mut command: Command, mode: Mode, deadline: Instant) -> Result<(DeadlineReader, Child)> {
    let (reader, writer) = output_stream(deadline)?;
    let (input, errors) = match mode {
        Mode::Queue => (
            Stdio::null(),
            writer
                .try_clone()
                .context(|| "opening the Codex error stream".to_string())?,
        ),
        Mode::Rpc(errors) => (Stdio::piped(), errors),
    };
    let child = command
        .stdin(input)
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::from(OwnedFd::from(errors)))
        .spawn()
        .context(|| "starting the Codex CLI".to_string())?;
    Ok((reader, child))
}

fn finish(child: &mut Child, deadline: Instant) -> Result<std::process::ExitStatus> {
    let limit = deadline.saturating_duration_since(Instant::now());
    if let Some(status) = child
        .wait_timeout(limit)
        .context(|| "waiting for the Codex CLI".to_string())?
    {
        return Ok(status);
    }
    stop(child)?;
    fail!("Codex did not exit within {} seconds.", WAIT.as_secs());
}

fn end(mut child: Child) -> Result<()> {
    if child
        .try_wait()
        .context(|| "checking the Codex CLI".to_string())?
        .is_none()
    {
        stop(&mut child)?;
    }
    Ok(())
}

fn stop(child: &mut Child) -> Result<()> {
    child
        .kill()
        .context(|| "stopping the Codex CLI".to_string())?;
    child
        .wait()
        .context(|| "waiting for the Codex CLI to stop".to_string())?;
    Ok(())
}

fn request(
    input: &mut impl Write,
    output: &mut impl BufRead,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let request = json!({"id": id, "method": method, "params": params});
    writeln!(input, "{request}")
        .and_then(|()| input.flush())
        .context(|| format!("sending Codex {method}"))?;
    let mut line = String::new();
    loop {
        line.clear();
        if output
            .read_line(&mut line)
            .context(|| format!("reading the Codex {method} response"))?
            == 0
        {
            fail!("Codex closed its connection before answering {method}.");
        }
        let response: Value =
            serde_json::from_str(&line).context(|| format!("reading Codex {method} JSON"))?;
        if response["id"].as_i64() == Some(id) {
            return Ok(response);
        }
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
