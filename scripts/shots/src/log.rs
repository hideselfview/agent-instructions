//! The program's lines on stderr: its notes, each after `builds shots: `, and
//! the lines of a check's outcome as they are, which builds keeps as a run's
//! findings.

use std::fmt::Display;

/// Writes `builds shots: <message>`.
pub(crate) fn note(message: impl Display) {
    eprintln!("builds shots: {message}");
}

/// Writes one line of a check's outcome as it is.
pub(crate) fn outcome(line: impl Display) {
    eprintln!("{line}");
}
