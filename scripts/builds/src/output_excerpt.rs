//! The lines of a step's output that say why it failed: from the first line
//! that starts a cause (a test's panic or a compiler's error), else the last
//! lines. A cargo run (cargo_run_outcome.rs) and a hook check
//! (hook_check_failure.rs) that fail keep one.

use std::collections::VecDeque;

/// The most lines an excerpt holds.
const LINES: usize = 20;

/// The most bytes an excerpt holds.
const BYTES: usize = 4000;

/// Does <line> start the part of the output that says why a step failed: a
/// test's panic or an error from rustc or cargo (`error:`, `error[E0308]:`)?
fn starts_cause(line: &str) -> bool {
    line.contains("panicked at") || line.starts_with("error:") || line.starts_with("error[")
}

/// Reads output line by line and keeps what an excerpt needs.
#[derive(Clone, Debug, Default)]
pub struct Excerpt {
    /// The lines from the first that starts a cause, at most `LINES`.
    cause: Vec<String>,
    /// The last `LINES` lines.
    last: VecDeque<String>,
}

impl Excerpt {
    /// The excerpt of all of <output>.
    pub fn of(output: &str) -> Option<String> {
        let mut excerpt = Excerpt::default();
        for line in output.lines() {
            excerpt.line(line);
        }
        excerpt.text()
    }

    /// Reads one line, without its newline.
    pub fn line(&mut self, text: &str) {
        if (self.cause.is_empty() && starts_cause(text))
            || (!self.cause.is_empty() && self.cause.len() < LINES)
        {
            self.cause.push(text.to_string());
        }
        self.last.push_back(text.to_string());
        if self.last.len() > LINES {
            self.last.pop_front();
        }
    }

    /// The lines from the first cause, else the last lines, cut to `BYTES`;
    /// None when they hold nothing but blanks.
    pub fn text(&self) -> Option<String> {
        let lines: Vec<&str> = if self.cause.is_empty() {
            self.last.iter().map(String::as_str).collect()
        } else {
            self.cause.iter().map(String::as_str).collect()
        };
        let mut text = lines.join("\n");
        if text.len() > BYTES {
            let mut cut = BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        (!text.trim().is_empty()).then_some(text)
    }
}

#[cfg(test)]
#[path = "output_excerpt_tests.rs"]
mod tests;
