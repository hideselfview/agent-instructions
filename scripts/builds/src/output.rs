//! Everything builds prints. Clippy refuses a print anywhere else in builds
//! (main.rs, clippy.toml). What builds prints is one of three things:
//!
//! - A message about the work: a note, a warning, progress, or a result for
//!   one file or checkout (`note`, `warn`, `say`, `Report`). How many there
//!   are grows with the work, so each kind, the messages one line of builds'
//!   code prints, gets LINES_PER_KIND lines in a command. `finish` then
//!   prints one total for each kind that had more. An agent reads what builds
//!   prints into its context, which one kind could otherwise fill.
//! - The output a caller asked for, such as help, rules, a list, a status,
//!   findings, JSON or a path (`answer`, `answer_line`, `answer_handle`). It
//!   prints whole: a caller that wants less of a list narrows it with the
//!   command's filters.
//! - What another program prints, which builds passes on as it is
//!   (`pass_through`, `relayed`, `child_stderr`, `inherit`).
//!
//! The caps count one process's lines, so the ledger that counts them is the
//! process's, as its stderr is. A process that serves until it is stopped,
//! the dashboard, writes a log that nothing ends with a total, so it keeps
//! every line (`keep_every_line`).

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::disallowed_methods,
    reason = "every print of builds is in this module"
)]

use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::Write;
use std::panic::Location;
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard};

use crate::fail::{Context, Result};
use crate::text::counted;

/// How many lines of one kind a command prints.
pub const LINES_PER_KIND: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

/// The line of builds' code that prints a kind: its file, line and column.
type Site = (&'static str, u32, u32);

#[track_caller]
fn caller() -> Site {
    let at = Location::caller();
    (at.file(), at.line(), at.column())
}

/// What a command has printed of one kind.
struct Kind {
    stream: Stream,
    printed: usize,
    left_out: usize,
    last_left_out: String,
}

struct Ledger {
    keep_every_line: bool,
    kinds: BTreeMap<Site, Kind>,
}

impl Ledger {
    const fn new() -> Ledger {
        Ledger {
            keep_every_line: false,
            kinds: BTreeMap::new(),
        }
    }

    /// The part of <message>, a message of the kind <site> prints, that the
    /// command prints now; the lines past the kind's cap are counted.
    fn admit(&mut self, site: Site, stream: Stream, message: &str) -> Option<String> {
        if self.keep_every_line {
            return Some(message.to_string());
        }
        let kind = self.kinds.entry(site).or_insert(Kind {
            stream,
            printed: 0,
            left_out: 0,
            last_left_out: String::new(),
        });
        let lines: Vec<&str> = message.split('\n').collect();
        let now = lines.len().min(LINES_PER_KIND - kind.printed);
        kind.printed += now;
        if let Some(last) = lines[now..].iter().rfind(|line| !line.is_empty()) {
            kind.left_out += lines.len() - now;
            kind.last_left_out = last.to_string();
        }
        (now > 0).then(|| lines[..now].join("\n"))
    }

    /// One total line for each kind that had more lines than it printed,
    /// each said once.
    fn take_totals(&mut self) -> Vec<(Stream, String)> {
        std::mem::take(&mut self.kinds)
            .into_values()
            .filter(|kind| kind.left_out > 0)
            .map(|kind| {
                let total = format!(
                    "builds: left out {} of one kind, the last of them: {}",
                    counted(kind.left_out, "more line", "more lines"),
                    kind.last_left_out
                );
                (kind.stream, total)
            })
            .collect()
    }
}

static LEDGER: Mutex<Ledger> = Mutex::new(Ledger::new());

fn ledger() -> MutexGuard<'static, Ledger> {
    LEDGER.lock().expect("no thread panics while it counts")
}

fn print_line(stream: Stream, line: &str) {
    match stream {
        Stream::Out => println!("{line}"),
        Stream::Err => eprintln!("{line}"),
    }
}

fn capped(site: Site, stream: Stream, message: &str) {
    let admitted = ledger().admit(site, stream, message);
    if let Some(lines) = admitted {
        print_line(stream, &lines);
    }
}

/// A message on stderr, `builds: <message>`, which agents and hooks read.
#[track_caller]
pub fn note(message: impl AsRef<str>) {
    capped(
        caller(),
        Stream::Err,
        &format!("builds: {}", message.as_ref()),
    );
}

/// A message on stderr in its own words, without the `builds:` of `note`.
#[track_caller]
pub fn warn(message: impl AsRef<str>) {
    capped(caller(), Stream::Err, message.as_ref());
}

/// A message on stdout.
#[track_caller]
pub fn say(message: impl AsRef<str>) {
    capped(caller(), Stream::Out, message.as_ref());
}

/// The messages of work that writes them to a place its caller picks, as a
/// landing does, capped like `say`'s.
pub struct Report<'a> {
    to: &'a mut dyn Write,
}

impl<'a> Report<'a> {
    pub fn to(to: &'a mut dyn Write) -> Report<'a> {
        Report { to }
    }

    #[track_caller]
    pub fn say(&mut self, message: impl AsRef<str>) {
        let admitted = ledger().admit(caller(), Stream::Out, message.as_ref());
        if let Some(lines) = admitted {
            self.pass_through(lines);
        }
    }

    /// Writes whole what another program printed, or the refusal that ends
    /// the work, which holds what the check or command that refused printed.
    pub fn pass_through(&mut self, output: impl Display) {
        // A reader that closed the stream wants no more of the report, and
        // the work goes on.
        let _ = writeln!(self.to, "{output}").and_then(|()| self.to.flush());
    }
}

/// Prints the totals of the kinds that had more lines than their cap. A
/// command calls it as it ends, or before another program takes its place.
pub fn finish() {
    let totals = ledger().take_totals();
    for (stream, total) in totals {
        print_line(stream, &total);
    }
}

/// Makes this process print every line: it serves until something stops it,
/// and what it prints is its log.
pub fn keep_every_line() {
    ledger().keep_every_line = true;
}

/// Output the caller asked for, as it is.
pub fn answer(text: impl Display) {
    print!("{text}");
}

/// A line of the output the caller asked for.
pub fn answer_line(line: impl Display) {
    println!("{line}");
}

/// Stdout, for a function that writes the output its caller asked for to a
/// handle.
pub fn answer_handle() -> std::io::Stdout {
    std::io::stdout()
}

/// A line on stderr that another builds process reads, beside the same line
/// of the answer on stdout.
pub fn answer_line_on_stderr(line: impl AsRef<str>) {
    eprintln!("{}", line.as_ref());
}

/// Another program's output, as it printed it, on stderr.
pub fn pass_through(output: impl AsRef<str>) {
    eprint!("{}", output.as_ref());
}

/// A line that a builds on a host printed, which capped its own lines there.
pub fn relayed(line: impl AsRef<str>) {
    eprintln!("{}", line.as_ref());
}

/// Stdout, for copying another program's output to it.
pub fn child_stdout() -> std::io::Stdout {
    std::io::stdout()
}

/// Stderr, for copying another program's output to it.
pub fn child_stderr() -> std::io::Stderr {
    std::io::stderr()
}

/// This process's stream, for a child to write to itself.
pub fn inherit() -> Stdio {
    Stdio::inherit()
}

/// This process's stderr, to give a child as its stdout: a command whose
/// stdout carries an answer (`builds tool` prints a folder) shows a child's
/// output on stderr.
pub fn stderr_for_child() -> Result<Stdio> {
    use std::os::fd::AsFd;
    let fd = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context(|| "duplicating stderr".to_string())?;
    Ok(Stdio::from(fd))
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
