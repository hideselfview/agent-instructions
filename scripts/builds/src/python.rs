//! The Python that a repo's steps run. Repos' `.checks` and `.app-build` lines
//! run `python3` scripts that import `tomllib`, which Python has from 3.11 on,
//! while macOS's own /usr/bin/python3 is 3.9. A command that runs steps
//! (`STEP_COMMANDS`) therefore puts a folder first on its own PATH, which
//! every step it starts inherits, and that folder's `python3` is:
//!
//!   - a link to the first Python of 3.11 or later that builds finds, when
//!     the first `python3` on PATH is older: the `python3` of each PATH folder
//!     and then of Homebrew's folders, then each folder's `python3.N`, the
//!     newest first;
//!   - a script that says which Python builds found and exits 1, when none of
//!     3.11 or later exists, so a step that runs `python3` fails with that
//!     message and not with an import error.
//!
//! When the first `python3` is 3.11 or later, PATH stays as it is.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::worktree::is_executable;

/// The commands that run a repo's steps, directly or through the builds they
/// start.
pub const STEP_COMMANDS: [&str; 6] = [
    "checks",
    "land",
    "app-build",
    "build-app",
    "programs",
    "new",
];

/// The oldest Python with `tomllib`.
const MINIMUM: (u32, u32) = (3, 11);

/// Folders searched after PATH's.
const EXTRA_FOLDERS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// Puts a folder with a `python3` of 3.11 or later first on this process's
/// PATH when the first `python3` on it is older. Call it before any thread
/// starts, since it writes the environment.
pub fn use_new_python(places: &Places) -> Result<()> {
    let Some(path) = std::env::var_os("PATH") else {
        fail!("PATH is not set, so builds cannot tell which python3 its steps run");
    };
    let extra: Vec<PathBuf> = EXTRA_FOLDERS.iter().map(PathBuf::from).collect();
    if let Some(path) = prepare(&path, &extra, &folder(places))? {
        // SAFETY: the caller runs before any thread starts, so none reads the
        // environment while set_var writes it.
        unsafe { std::env::set_var("PATH", path) };
    }
    Ok(())
}

/// The folder of the `python3` that steps run, in <places>' cache.
fn folder(places: &Places) -> PathBuf {
    places.cache_home.join("builds").join("python")
}

/// The PATH for steps started with <path>: None when its first `python3` is
/// 3.11 or later, else <path> with <folder> first, after making <folder>'s
/// `python3` (see the module's text). <extra> are searched after <path>'s
/// folders.
fn prepare(path: &OsStr, extra: &[PathBuf], folder: &Path) -> Result<Option<OsString>> {
    let mut problems = Vec::new();
    let first = crate::install::on_path_in("python3", path);
    let first_said = match &first {
        Some(first) => match version(first) {
            Ok(found) if found >= MINIMUM => return Ok(None),
            Ok((major, minor)) => format!("{} is Python {major}.{minor}", first.display()),
            Err(why) => {
                problems.push(why);
                format!("{} did not say its version", first.display())
            }
        },
        None => "PATH has no python3".to_string(),
    };
    let found = candidates(path, extra, folder, &mut problems)
        .into_iter()
        .filter(|candidate| Some(candidate) != first.as_ref())
        .find(|candidate| match version(candidate) {
            Ok(found) => found >= MINIMUM,
            Err(why) => {
                problems.push(why);
                false
            }
        });
    std::fs::create_dir_all(folder).context(|| format!("making {}", folder.display()))?;
    let python3 = folder.join("python3");
    match found {
        Some(found) => {
            for problem in &problems {
                crate::output::note(format!("python: {problem}"));
            }
            crate::built_tools::point(&python3, &found)?
        }
        None => {
            let mut why = format!(
                "builds runs Python steps with Python {}.{} or later, which has tomllib, and \
                 found none: {first_said}, and no python3 or python3.N of {}.{} or later is in \
                 PATH or in {}. Install one (`brew install python`).",
                MINIMUM.0,
                MINIMUM.1,
                MINIMUM.0,
                MINIMUM.1,
                extra
                    .iter()
                    .map(|folder| folder.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for problem in problems {
                why.push_str(&format!(" {problem}."));
            }
            refusing_script(&python3, &why)?;
        }
    }
    Ok(Some(crate::programs::path_with_first(folder, Some(path))))
}

/// The executables that may be a Python of 3.11 or later, in the order to try
/// them: in each folder of <path> and then <extra>, `python3`, then each
/// `python3.N` the newest first. <folder> is the one this module makes, so its
/// `python3` is no candidate. A folder that cannot be listed adds its error to
/// <problems>, which `prepare` says on stderr when it links a Python anyway.
fn candidates(
    path: &OsStr,
    extra: &[PathBuf],
    folder: &Path,
    problems: &mut Vec<String>,
) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut found = Vec::new();
    for dir in std::env::split_paths(path).chain(extra.iter().cloned()) {
        if dir == folder || seen.contains(&dir) {
            continue;
        }
        let entries = crate::capped_store::entries_in(&dir).unwrap_or_else(|error| {
            problems.push(error.text());
            Vec::new()
        });
        let mut versioned: Vec<(u32, PathBuf)> = entries
            .into_iter()
            .filter_map(|(name, path)| {
                let minor = name.strip_prefix("python3.")?.parse::<u32>().ok()?;
                (minor >= MINIMUM.1 && is_executable(&path)).then_some((minor, path))
            })
            .collect();
        versioned.sort_by_key(|(minor, _)| std::cmp::Reverse(*minor));
        let plain = dir.join("python3");
        if is_executable(&plain) {
            found.push(plain);
        }
        found.extend(versioned.into_iter().map(|(_, path)| path));
        seen.push(dir);
    }
    found
}

/// The (major, minor) version of the Python <program>; fails saying why it
/// could not run or did not answer.
fn version(program: &Path) -> std::result::Result<(u32, u32), String> {
    let mut command = Command::new(program);
    command.args([
        "-c",
        "import sys; print(sys.version_info[0], sys.version_info[1])",
    ]);
    let text = crate::host::output_of(command, &program.display().to_string())?;
    let mut words = text.split_whitespace().map(str::parse::<u32>);
    match (words.next(), words.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => Ok((major, minor)),
        _ => Err(format!(
            "{} did not print its version, but {:?}",
            program.display(),
            text.trim()
        )),
    }
}

/// Makes <file> a script that prints <why> to stderr and exits 1, replacing
/// whatever is there in one rename.
fn refusing_script(file: &Path, why: &str) -> Result<()> {
    let script = format!(
        "#!/bin/sh\necho {} >&2\nexit 1\n",
        crate::text::shell_word(why)
    );
    if std::fs::read_to_string(file).is_ok_and(|now| now == script) {
        return Ok(());
    }
    crate::whole_file::write_executable(file, script)
}

#[cfg(test)]
#[path = "python_tests.rs"]
mod tests;
