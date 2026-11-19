//! The names builds runs under besides its own. Each is a symlink to builds
//! in `~/.local/bin`, which `builds install` and `builds host setup` make,
//! and builds started by one of those names runs the program the name stands
//! for (`run_as`).

use std::convert::Infallible;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::fail::Result;

/// A name builds runs under, and what it runs then.
pub struct Link {
    pub name: &'static str,
    /// Runs the program with its arguments in this process; returns only
    /// when it cannot.
    pub run: fn(&[OsString]) -> Result<Infallible>,
}

/// Every name builds runs under besides its own.
pub const LINKS: [Link; 2] = [
    Link {
        name: crate::linker::NAME,
        run: crate::linker::run,
    },
    Link {
        name: crate::rustc_wrapper::NAME,
        run: crate::rustc_wrapper::run,
    },
];

/// The folder that holds builds and its links on a Mac whose home is <home>.
pub fn bin(home: &Path) -> PathBuf {
    home.join(".local/bin")
}

/// The path of the program <name> in `bin`.
pub fn path(home: &Path, name: &str) -> PathBuf {
    bin(home).join(name)
}

/// Runs the program that <program>, the path builds was started by, names
/// when its file name is one of `LINKS`, with <args>, and fails when it
/// cannot. None when builds was started by another name.
pub fn run_as(program: &OsStr, args: &[OsString]) -> Option<crate::fail::Fail> {
    let name = Path::new(program).file_name()?;
    let link = LINKS.iter().find(|link| OsStr::new(link.name) == name)?;
    let Err(fail) = (link.run)(args);
    Some(fail)
}
