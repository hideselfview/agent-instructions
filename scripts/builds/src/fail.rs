//! How a command ends when it can't go on: with a message the tool prints as
//! `builds: <message>` and exit status 1, or with another program's exit
//! status passed through (`builds cargo`), which that program already
//! explained.

use std::fmt::Display;

#[derive(Debug)]
pub enum Fail {
    Message(String),
    Exit(i32),
}

impl Fail {
    /// What to say of it: the message, or the exit status.
    pub fn text(self) -> String {
        match self {
            Fail::Message(message) => message,
            Fail::Exit(code) => format!("exit {code}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Fail>;

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
    match (first, then) {
        (Err(error), Err(also)) => {
            crate::text::note(also.text());
            Err(error)
        }
        (first, then) => {
            then?;
            first
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(result: Result<u8>) -> String {
        result.expect_err("fails").text()
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
