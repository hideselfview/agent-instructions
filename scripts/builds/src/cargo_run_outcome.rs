//! How a `builds cargo` run ended (cargo_run_outcome.sql): passed, failed or
//! stopped, the tests it reported failed, and an excerpt of its output that
//! says why. A `Watch` reads each line of cargo's output as builds passes it
//! through (`Tee`), and cargo_run.rs writes the outcome with the run.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;

/// The most lines an excerpt holds.
const EXCERPT_LINES: usize = 20;

/// The most bytes an excerpt holds.
const EXCERPT_BYTES: usize = 4000;

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Passed,
    Failed,
    Stopped,
}

impl Ended {
    pub fn word(self) -> &'static str {
        match self {
            Ended::Passed => "passed",
            Ended::Failed => "failed",
            Ended::Stopped => "stopped",
        }
    }

    /// How a run whose cargo exited with <code> ended: a code above 128 is
    /// a signal's (`fail::exit_code`).
    pub fn of_code(code: i32) -> Ended {
        match code {
            0 => Ended::Passed,
            129.. => Ended::Stopped,
            _ => Ended::Failed,
        }
    }
}

/// How a run ended, the tests it reported failed and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub ended: Ended,
    /// In the order cargo first reported them.
    pub failed: Vec<String>,
    pub excerpt: Option<String>,
}

impl Outcome {
    /// A run that passed and printed nothing, for tests that record runs.
    #[cfg(test)]
    pub fn passed() -> Outcome {
        Outcome {
            ended: Ended::Passed,
            failed: Vec::new(),
            excerpt: None,
        }
    }
}

/// What a `Watch` read of cargo's output.
#[derive(Default)]
struct Read {
    failed: Vec<String>,
    /// The lines from the first test panic or compiler error, at most
    /// `EXCERPT_LINES`.
    cause: Vec<String>,
    /// The last `EXCERPT_LINES` lines.
    last: VecDeque<String>,
    /// Did any line pass through?
    any: bool,
}

/// Reads cargo's output line by line, from every stream builds passes
/// through. Clones share what they read.
#[derive(Clone, Default)]
pub struct Watch(Arc<Mutex<Read>>);

/// The test a line of cargo test's or nextest's output reports failed: `test
/// a::b ... FAILED` from cargo test, `FAIL [ 0.1s] (1/5) crate a::b` or
/// `TIMEOUT [ ...` from nextest, whose test name is the line's last word.
pub fn failed_test(line: &str) -> Option<&str> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("test ")
        && let Some(name) = rest.strip_suffix(" ... FAILED")
    {
        return Some(name.trim());
    }
    let rest = line
        .strip_prefix("FAIL [")
        .or_else(|| line.strip_prefix("TIMEOUT ["))
        .or_else(|| line.strip_prefix("SIGSEGV ["))
        .or_else(|| line.strip_prefix("SIGABRT ["))?;
    let (_, after) = rest.split_once(']')?;
    after.split_whitespace().last()
}

/// Does <line> start the part of the output that says why the run failed: a
/// test's panic or a compiler error?
fn starts_cause(line: &str) -> bool {
    line.contains("panicked at") || line.starts_with("error:") || line.starts_with("error[")
}

impl Watch {
    /// Reads one line of output, its newline included or not.
    pub fn line(&self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches(['\n', '\r']);
        let mut read = self.0.lock().expect("no panic holding it");
        read.any = true;
        if let Some(test) = failed_test(text)
            && !read.failed.iter().any(|known| known == test)
        {
            read.failed.push(test.to_string());
        }
        if (read.cause.is_empty() && starts_cause(text))
            || (!read.cause.is_empty() && read.cause.len() < EXCERPT_LINES)
        {
            read.cause.push(text.to_string());
        }
        read.last.push_back(text.to_string());
        if read.last.len() > EXCERPT_LINES {
            read.last.pop_front();
        }
    }

    /// How the run ended, given cargo's exit <code>.
    pub fn outcome(&self, code: i32) -> Outcome {
        let read = self.0.lock().expect("no panic holding it");
        let ended = Ended::of_code(code);
        let lines: Vec<&str> = if read.cause.is_empty() {
            read.last.iter().map(String::as_str).collect()
        } else {
            read.cause.iter().map(String::as_str).collect()
        };
        let mut excerpt = lines.join("\n");
        if excerpt.len() > EXCERPT_BYTES {
            let mut cut = EXCERPT_BYTES;
            while !excerpt.is_char_boundary(cut) {
                cut -= 1;
            }
            excerpt.truncate(cut);
        }
        Outcome {
            ended,
            failed: read.failed.clone(),
            excerpt: (ended != Ended::Passed && read.any && !excerpt.trim().is_empty())
                .then_some(excerpt),
        }
    }
}

/// A writer that passes every byte on to <inner> and each whole line to a
/// `Watch`.
pub struct Tee<W: Write> {
    inner: W,
    watch: Watch,
    partial: Vec<u8>,
}

impl<W: Write> Tee<W> {
    pub fn new(inner: W, watch: Watch) -> Tee<W> {
        Tee {
            inner,
            watch,
            partial: Vec::new(),
        }
    }
}

impl<W: Write> Tee<W> {
    /// Gives each whole line of <buf> to the watch, then, when <print> is
    /// set, writes <buf> to the inner writer: the watch reads every line
    /// even after a write failed.
    pub fn write_all_watched(&mut self, buf: &[u8], print: bool) -> std::io::Result<()> {
        self.watch_bytes(buf);
        if print {
            self.inner.write_all(buf)?;
            self.inner.flush()?;
        }
        Ok(())
    }

    fn watch_bytes(&mut self, buf: &[u8]) {
        for byte in buf {
            self.partial.push(*byte);
            if *byte == b'\n' {
                self.watch.line(&self.partial);
                self.partial.clear();
            }
        }
    }
}

impl<W: Write> Write for Tee<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.watch_bytes(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Drop for Tee<W> {
    fn drop(&mut self) {
        if !self.partial.is_empty() {
            self.watch.line(&self.partial);
        }
    }
}

/// Records how run <run> ended.
pub fn record(db: &Connection, run: i64, outcome: &Outcome) -> Result<()> {
    db.execute(
        "INSERT INTO builds_cargo_run_outcome (run_id, outcome, excerpt) VALUES (?1, ?2, ?3)",
        params![run, outcome.ended.word(), outcome.excerpt],
    )
    .sql()?;
    for test in &outcome.failed {
        db.execute(
            "INSERT INTO builds_cargo_run_failed_test (run_id, test) VALUES (?1, ?2)",
            params![run, test],
        )
        .sql()?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "cargo_run_outcome_tests.rs"]
mod tests;
