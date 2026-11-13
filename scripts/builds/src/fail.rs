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
