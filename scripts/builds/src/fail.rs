//! How a command ends when it can't go on: with a message the tool prints as
//! `builds: <message>` and exit status 1, or with another program's exit
//! status passed through (`builds cargo`), which that program already
//! explained.

use std::fmt::Display;

#[derive(Debug)]
pub enum Fail {
    Message(String),
    Exit(i32),
    Move {
        from: std::path::PathBuf,
        to: std::path::PathBuf,
        error: std::io::Error,
    },
}

impl Fail {
    /// What to say of it: the message, or the exit status.
    pub fn text(self) -> String {
        self.to_string()
    }
}

impl Display for Fail {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => output.write_str(message),
            Self::Exit(code) => write!(output, "exit {code}"),
            Self::Move { from, to, error } => {
                write!(
                    output,
                    "moving {} to {}: {error}",
                    from.display(),
                    to.display()
                )
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, Fail>;

impl From<boards::Error> for Fail {
    fn from(error: boards::Error) -> Fail {
        Fail::Message(error.text())
    }
}

/// The exit code a shell gives for a program that ended with <status>: its
/// own code, or 128 plus the signal that ended it.
pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Is <error> what reading a path gives when the path does not exist: it, or
/// a folder above it, is missing or is a file (as when it was deleted while
/// it was being read)?
pub fn gone(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Ends the current function with a `Fail::Message` built like `format!`.
#[macro_export]
macro_rules! fail {
    ($($arg:tt)*) => {
        return Err($crate::fail::Fail::Message(format!($($arg)*)))
    };
}

/// Names what was being done when an error from the system happened.
pub trait Context<T> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E: Display> Context<T> for std::result::Result<T, E> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|error| Fail::Message(format!("{}: {error}", what())))
    }
}

/// The end of a step <first> and the cleanup <then> that ran after it however
/// it ended: <first>'s error when it failed, with <then>'s error noted on
/// stderr when that failed too; else <then>'s error; else <first>'s value.
pub fn with_cleanup<T>(first: Result<T>, then: Result<()>) -> Result<T> {
    finish_cleanup(first, then, |_, _| false)
}

/// Setup may continue after a refused move, but it must stop on a
/// separate cleanup failure. Keep both diagnostics when both steps fail.
pub fn with_move_cleanup<T>(moved: Result<T>, cleanup: Result<()>) -> Result<T> {
    finish_cleanup(moved, cleanup, |first, then| {
        matches!(first, Fail::Move { .. }) && !matches!(then, Fail::Move { .. })
    })
}

fn finish_cleanup<T>(
    first: Result<T>,
    then: Result<()>,
    prefer_cleanup: impl FnOnce(&Fail, &Fail) -> bool,
) -> Result<T> {
    match (first, then) {
        (Err(first), Err(then)) => {
            let (reported, returned) = if prefer_cleanup(&first, &then) {
                (first, then)
            } else {
                (then, first)
            };
            crate::text::note(reported.text());
            Err(returned)
        }
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(result: Result<u8>) -> String {
        result.expect_err("fails").text()
    }

    #[test]
    fn an_exit_code_is_the_programs_code_or_128_plus_its_signal() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;
        assert_eq!(exit_code(ExitStatus::from_raw(0)), 0);
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code(ExitStatus::from_raw(libc::SIGKILL)), 137);
    }

    #[test]
    fn a_step_and_its_cleanup_end_with_the_first_error_there_is() {
        fn failed<T>(text: &str) -> Result<T> {
            Err(Fail::Message(text.to_string()))
        }
        assert_eq!(with_cleanup(Ok(7), Ok(())).unwrap(), 7);
        assert_eq!(message(with_cleanup(failed("step"), Ok(()))), "step");
        assert_eq!(message(with_cleanup(Ok(7), failed("cleanup"))), "cleanup");
        assert_eq!(
            message(with_cleanup(failed("step"), failed("cleanup"))),
            "step"
        );
    }
}
