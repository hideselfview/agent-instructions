//! A repo's design files, as builds and the programs it runs read them: the
//! boards in `design/mocks`, the canvas index that places and sizes them, and
//! the design system in `design/system` with the tokens.css and bundles that
//! the boards load. Each is read from a `Source`: a folder's files as they
//! are, or the files at a commit, read with git without checking anything
//! out. Nothing here writes into a repo's files.

pub mod app_build;
pub mod board;
pub mod colors;
pub mod component_key;
pub mod fonts;
pub mod preview;
pub mod routes;
pub mod runtime;
pub mod segment;
pub mod source;
pub mod tokens;

pub use board::{
    BOARD_SUFFIX, CANVAS, MOCKS, board_name, board_path, canvas_place, canvas_size,
    design_namespace, imports,
};
pub use source::{Source, exists_in, export, git, git_text, in_scratch};
pub use tokens::{TOKENS_CSS, TOKENS_JSON, tokens_css};

use std::fmt;
use std::path::PathBuf;

/// The folders a board's page is built from: the cache the design systems
/// generated for commits are kept in, and the folder of the SF Mono files
/// its tokens.css has rules for (`fonts`); none on a host that is not a Mac,
/// whose boards draw the next family of their mono stack. The run also keeps
/// the slot files that limit the pages Chromes draw at once in the build
/// volume (`volume`).
#[derive(Clone, Debug)]
pub struct Folders {
    pub cache: PathBuf,
    pub fonts: Option<PathBuf>,
    pub volume: PathBuf,
}

/// Why reading a repo's design files failed, as a sentence that names the
/// file or the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error(message.into())
    }

    /// The sentence.
    pub fn text(self) -> String {
        self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Ends the current function with an `Error` built like `format!`.
macro_rules! fail {
    ($($arg:tt)*) => {
        return Err($crate::Error::new(format!($($arg)*)))
    };
}
pub(crate) use fail;

/// Names what was being done when an error from the system happened.
pub(crate) trait Context<T> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|error| Error(format!("{}: {error}", what())))
    }
}

/// <text> with `&`, `<`, `>` and `"` written as HTML text and attribute
/// values hold them.
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod testing;
