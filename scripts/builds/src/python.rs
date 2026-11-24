//! The Python that a repo's steps run. Repos' `.checks` and `.app-build` lines
//! run `python3 scripts/x.py`, and some of those scripts import `tomllib`,
//! which Python has from 3.11 on, while macOS's own /usr/bin/python3 is 3.9.
//! So a step runs with a Python of 3.11 or later first on its PATH, whichever
//! `python3` the agent's PATH names first.
//!
//! When the first `python3` on PATH is 3.11 or later, the step's PATH stays as
//! it is. Otherwise builds looks for the first `python3` or `python3.N` of
//! 3.11 or later in the PATH's folders and in Homebrew's folders, links it as
//! `python3` in `~/.local/lib/builds/python/`, and puts that folder first on
//! the step's PATH. With none, a step whose program is a Python fails, naming
//! the version it found; any other step runs with the PATH it has.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::fail;
use crate::fail::{Context, Result};

/// The oldest Python with `tomllib`.
const MINIMUM: (u32, u32) = (3, 11);

/// The newest minor version that builds looks for by name.
const NEWEST_MINOR: u32 = 20;

/// Folders searched after PATH's.
const EXTRA_FOLDERS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// The folder of the `python3` link, relative to the home folder.
const FOLDER: &str = ".local/lib/builds/python";

/// The PATH a step whose program is <program> runs with, given the <path> it
/// would run with otherwise: None to keep <path>, or <path> with a Python of
/// 3.11 or later first.
/// Fails when <program> is a Python and none of 3.11 or later exists.
pub fn path_for_step(program: &str, path: &OsStr) -> Result<Option<OsString>> {
    static CHOICE: OnceLock<std::result::Result<Choice, String>> = OnceLock::new();
    let choice = CHOICE.get_or_init(|| {
        let path = std::env::var_os("PATH");
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let extra: Vec<PathBuf> = EXTRA_FOLDERS.iter().map(PathBuf::from).collect();
        choose(path.as_deref(), &extra, home.as_deref()).map_err(|error| error.text())
    });
    match choice {
        Ok(Choice::OnPath) => Ok(None),
        Ok(Choice::Linked(folder)) => Ok(Some(with_first(folder, path))),
        Err(why) if is_python(program) => fail!("{why}"),
        Err(_) => Ok(None),
    }
}

/// Whether <program> names a Python.
fn is_python(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program);
    name == "python" || name.starts_with("python3")
}

fn with_first(folder: &Path, path: &OsStr) -> OsString {
    let mut joined = OsString::from(folder);
    if !path.is_empty() {
        joined.push(":");
        joined.push(path);
    }
    joined
}

#[derive(Debug, PartialEq, Eq)]
enum Choice {
    /// The first `python3` on PATH is new enough.
    OnPath,
    /// The folder whose `python3` is a new enough Python.
    Linked(PathBuf),
}

/// Chooses the Python for steps run with <path>, searching <extra> after it,
/// and links it in <home>'s folder.
fn choose(path: Option<&OsStr>, extra: &[PathBuf], home: Option<&Path>) -> Result<Choice> {
    let folders: Vec<PathBuf> = path
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default();
    let first = folders
        .iter()
        .map(|folder| folder.join("python3"))
        .find(|candidate| candidate.is_file());
    let first_version = first.as_deref().and_then(version);
    if first_version.is_some_and(|found| found >= MINIMUM) {
        return Ok(Choice::OnPath);
    }
    let found = folders
        .iter()
        .chain(extra)
        .flat_map(|folder| names().map(move |name| folder.join(name)))
        .filter(|candidate| candidate.is_file())
        .find(|candidate| version(candidate).is_some_and(|found| found >= MINIMUM));
    let Some(found) = found else {
        let seen = match (&first, first_version) {
            (Some(first), Some((major, minor))) => {
                format!("{} is Python {major}.{minor}", first.display())
            }
            (Some(first), None) => format!("{} did not say its version", first.display()),
            (None, _) => "PATH has no python3".to_string(),
        };
        fail!(
            "builds runs Python steps with Python {}.{} or later, which has tomllib, and found \
             none: {seen}, and no python3.{}..python3.{NEWEST_MINOR} exists in PATH or in {}. \
             Install one (`brew install python`).",
            MINIMUM.0,
            MINIMUM.1,
            MINIMUM.1,
            extra
                .iter()
                .map(|folder| folder.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    let Some(home) = home else {
        fail!("HOME is not set, so builds cannot link {}", found.display());
    };
    Ok(Choice::Linked(link(home, &found)?))
}

/// `python3`, then `python3.N` from the newest to the oldest that has tomllib.
fn names() -> impl Iterator<Item = String> {
    std::iter::once("python3".to_string()).chain(
        (MINIMUM.1..=NEWEST_MINOR)
            .rev()
            .map(|minor| format!("python3.{minor}")),
    )
}

/// The (major, minor) version of the Python <program>, None when it does not
/// say.
fn version(program: &Path) -> Option<(u32, u32)> {
    let output = Command::new(program)
        .args([
            "-c",
            "import sys; print(sys.version_info[0], sys.version_info[1])",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut words = text.split_whitespace().map(str::parse::<u32>);
    match (words.next(), words.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => Some((major, minor)),
        _ => None,
    }
}

/// Makes `<home>/.local/lib/builds/python/python3` a link to <python>, and
/// returns the folder. A link that already names <python> stays; another is
/// replaced in one rename, so a step starting meanwhile finds one or the
/// other.
fn link(home: &Path, python: &Path) -> Result<PathBuf> {
    let folder = home.join(FOLDER);
    std::fs::create_dir_all(&folder).context(|| format!("making {}", folder.display()))?;
    let link = folder.join("python3");
    if std::fs::read_link(&link).is_ok_and(|target| target == python) {
        return Ok(folder);
    }
    let temporary = folder.join(format!("python3.{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    std::os::unix::fs::symlink(python, &temporary)
        .context(|| format!("linking {}", temporary.display()))?;
    std::fs::rename(&temporary, &link).context(|| format!("replacing {}", link.display()))?;
    Ok(folder)
}

#[cfg(test)]
#[path = "python_tests.rs"]
mod tests;
