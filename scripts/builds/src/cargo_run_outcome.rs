//! How a `builds cargo` run ended (cargo_run_outcome.sql): passed, failed or
//! stopped, the tests it reported failed, and an excerpt of its output that
//! says why (output_excerpt.rs). A `Watch` reads each line of cargo's output as builds passes it
//! through (`Tee`), and cargo_run.rs writes the outcome with the run.

use std::io::Write;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::output_excerpt::Excerpt;
use crate::run_end::RunEnd;

/// How a run ended, the tests it reported failed and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub ended: RunEnd,
    /// In the order cargo first reported them.
    pub failed: Vec<String>,
    pub excerpt: Option<String>,
}

impl Outcome {
    /// A run that passed and printed nothing, for tests that record runs.
    #[cfg(test)]
    pub fn passed() -> Outcome {
        Outcome {
            ended: RunEnd::Passed,
            failed: Vec::new(),
            excerpt: None,
        }
    }
}

/// What a `Watch` read of cargo's output.
#[derive(Default)]
struct Read {
    failed: Vec<String>,
    excerpt: Excerpt,
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

impl Watch {
    /// Reads one line of output, its newline included or not.
    pub fn line(&self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches(['\n', '\r']);
        let mut read = self.0.lock().expect("no panic holding it");
        if let Some(test) = failed_test(text)
            && !read.failed.iter().any(|known| known == test)
        {
            read.failed.push(test.to_string());
        }
        read.excerpt.line(text);
    }

    /// How the run ended, given cargo's exit <code>.
    pub fn outcome(&self, code: i32) -> Outcome {
        let read = self.0.lock().expect("no panic holding it");
        let ended = RunEnd::of_code(code);
        Outcome {
            ended,
            failed: read.failed.clone(),
            excerpt: (ended != RunEnd::Passed)
                .then(|| read.excerpt.text())
                .flatten(),
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
