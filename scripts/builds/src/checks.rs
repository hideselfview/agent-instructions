//! A repo's `.checks`: the commit checks the repo declares, one per line,
//! the way `.app-build` declares app build steps:
//!
//! ```text
//! check fmt *.rs cargo fmt --check
//! fix swift-format *.swift swift-format format --in-place {files}
//! check clippy *.rs,Cargo.toml,Cargo.lock cargo clippy {crates} -- -D warnings
//! whole frozen-lists owner.toml,scripts/check-frozen-shrinks.py python3 scripts/check-frozen-shrinks.py
//! test loc-words scripts/loc-words.py,spec/words.md scripts/loc-words-test.sh
//! ```
//!
//! `<kind> <name> <paths> <program> [<args>...]`. A `whole` line is a `check`
//! that reads the whole tree and never takes `{files}` or `{crates}`: the hook
//! runs it when a path matches, like any check, and `builds land` also runs it
//! on every landing, since two commits that each pass the hook can break it
//! together. The program is not run by a shell. `checks_run.rs` picks the lines a set of changed files matches and
//! runs them; this file reads the lines and expands what a match hands the
//! program.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::checkout::git_local_env_vars;
use crate::fail;
use crate::fail::{Context, Result};
use crate::glob::{Question, segment_matches, segments_match};

pub const CONFIG: &str = ".checks";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Kind {
    /// A static check, run when a staged file matches.
    Check,
    /// A formatter, run the same way; the files it changed are staged again.
    Fix,
    /// A checker's own test; the hook never runs it, CI does.
    Test,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Check => "check",
            Kind::Fix => "fix",
            Kind::Test => "test",
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Line {
    pub kind: Kind,
    pub name: String,
    /// The files the line reads.
    pub paths: Vec<PathGlob>,
    /// The program, relative to the repo when it names a path, and its
    /// arguments, `{files}` and `{crates}` not yet expanded.
    pub command: Vec<String>,
    /// A `whole` line: a static check of the whole tree, which `builds land`
    /// runs on every landing. Its kind is `Check`.
    pub whole: bool,
}

/// One glob of a line's paths, over repo-relative paths.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PathGlob {
    pub glob: String,
    /// Written `every:<glob>`: a matching file configures the check of
    /// every crate, as the source of a lint driver the line runs does, so it
    /// hands `{crates}` every crate.
    pub every_crate: bool,
}

/// The prefix of a glob whose files count as every crate.
const EVERY_CRATE: &str = "every:";

/// A line that a set of changed files picked, with what it runs.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Planned {
    pub line: Line,
    /// The changed files that match its paths and exist.
    pub files: Vec<String>,
    /// The command with its placeholders expanded.
    pub argv: Vec<String>,
}

pub fn parse(text: &str) -> Result<Vec<Line>> {
    let mut lines: Vec<Line> = Vec::new();
    for raw in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let mut words = raw.split_whitespace().map(str::to_string);
        let (Some(kind), Some(name), Some(paths)) = (words.next(), words.next(), words.next())
        else {
            fail!("{CONFIG}: {raw:?} needs a kind, a name, paths and a command");
        };
        let whole = kind == "whole";
        let kind = match kind.as_str() {
            "check" | "whole" => Kind::Check,
            "fix" => Kind::Fix,
            "test" => Kind::Test,
            other => {
                fail!("{CONFIG}: {raw:?} starts with {other:?}, not check, whole, fix or test")
            }
        };
        let command: Vec<String> = words.collect();
        if command.is_empty() {
            fail!("{CONFIG}: {} {name} names no command", kind.word());
        }
        if whole
            && command
                .iter()
                .any(|arg| arg == "{files}" || arg == "{crates}")
        {
            fail!(
                "{CONFIG}: whole {name} names {{files}} or {{crates}}, but a whole check reads the whole tree; make it a check, or drop the placeholder"
            );
        }
        if lines.iter().any(|line| line.name == name) {
            fail!("{CONFIG}: {name} is named twice");
        }
        let paths: Vec<PathGlob> = paths
            .split(',')
            .filter(|p| !p.is_empty())
            .map(|p| match p.strip_prefix(EVERY_CRATE) {
                Some(glob) => PathGlob {
                    glob: glob.to_string(),
                    every_crate: true,
                },
                None => PathGlob {
                    glob: p.to_string(),
                    every_crate: false,
                },
            })
            .collect();
        if paths.is_empty() {
            fail!("{CONFIG}: {name} names no paths");
        }
        if paths.iter().any(|p| p.glob.is_empty()) {
            fail!("{CONFIG}: {name} writes {EVERY_CRATE} with no glob after it");
        }
        if paths.iter().any(|p| p.every_crate) && !command.iter().any(|arg| arg == "{crates}") {
            fail!(
                "{CONFIG}: {name} marks paths {EVERY_CRATE}, which hand {{crates}} every crate, but names no {{crates}}"
            );
        }
        lines.push(Line {
            kind,
            name,
            paths,
            command,
            whole,
        });
    }
    Ok(lines)
}

/// Whether <path> (repo-relative, `/`-separated) matches <pattern>: `*` stays
/// inside one path segment, a `**` segment is any number of segments, and a
/// pattern with no `/` is matched against the file name wherever it sits.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let path: Vec<&str> = path.split('/').collect();
    if !pattern.contains('/') {
        return path
            .last()
            .is_some_and(|name| segment_matches(pattern, name, Question::Plain));
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    segments_match(&pattern, &path, Question::Plain)
}

impl Line {
    pub fn matches(&self, path: &str) -> bool {
        self.paths.iter().any(|p| glob_match(&p.glob, path))
    }

    /// Does an `every:` glob of the line match <path>?
    fn counts_as_every_crate(&self, path: &str) -> bool {
        self.paths
            .iter()
            .any(|p| p.every_crate && glob_match(&p.glob, path))
    }

    fn uses(&self, placeholder: &str) -> bool {
        self.command.iter().any(|arg| arg == placeholder)
    }
}

/// The lines of <kinds> that some of <files> match, each with its command
/// expanded, in file order. A line that names `{files}` or `{crates}` and has
/// nothing to hand it (no matching file still there, no crate holding one) is
/// left out. With <every>, a line is picked whether or not a file matches it.
/// <files> changed from commit <base> to the working tree: a changed root
/// `Cargo.lock` hands `{crates}` the crates whose resolved dependencies
/// changed since <base> (`lock_diff`), or every crate with no <base>.
pub fn plan(
    top: &Path,
    lines: &[Line],
    kinds: &[Kind],
    files: &[String],
    every: bool,
    base: Option<&str>,
) -> Result<Vec<Planned>> {
    // Planning asks cargo for the workspace's crates, and the lines the plan
    // picks run cargo and rustc.
    ensure_toolchain(top)?;
    let mut metadata: Option<Vec<Crate>> = None;
    let mut lock: Option<Vec<String>> = None;
    let mut planned = Vec::new();
    for line in lines.iter().filter(|line| kinds.contains(&line.kind)) {
        let matching: Vec<String> = files
            .iter()
            .filter(|file| line.matches(file))
            .cloned()
            .collect();
        if matching.is_empty() && !every {
            continue;
        }
        let existing: Vec<String> = matching
            .into_iter()
            .filter(|file| top.join(file).exists())
            .collect();
        let crates = if line.uses("{crates}") {
            if metadata.is_none() {
                metadata = Some(workspace_crates(top)?);
            }
            let crates = metadata.as_deref().expect("just read");
            if lock.is_none() && existing.iter().any(|file| file == LOCK) {
                lock = Some(lock_crates(top, crates, base)?);
            }
            let held = crates_holding(line, crates, &existing, lock.as_deref().unwrap_or_default());
            if held.is_empty() {
                continue;
            }
            held
        } else {
            Vec::new()
        };
        if line.uses("{files}") && existing.is_empty() {
            continue;
        }
        let mut argv = Vec::new();
        for arg in &line.command {
            match arg.as_str() {
                "{files}" => argv.extend(existing.iter().cloned()),
                "{crates}" => argv.extend(
                    crates
                        .iter()
                        .flat_map(|name| ["-p".to_string(), name.clone()]),
                ),
                _ => argv.push(arg.clone()),
            }
        }
        planned.push(Planned {
            line: line.clone(),
            files: existing,
            argv,
        });
    }
    Ok(planned)
}

struct Crate {
    name: String,
    /// The crate's folder relative to the repo, empty for the repo itself.
    dir: String,
}

/// Installs the toolchain the checkout at <top> pins when this machine lacks
/// it: the checks run cargo and rustc, and rustup installs nothing for them.
fn ensure_toolchain(top: &Path) -> Result<()> {
    match crate::rust_toolchain::in_checkout(top, top)? {
        Some(toolchain) => crate::rust_toolchain::ensure(
            &crate::places::Places::from_env()?,
            &crate::system::System::from_env()?,
            &toolchain,
        ),
        None => Ok(()),
    }
}

fn workspace_crates(top: &Path) -> Result<Vec<Crate>> {
    let mut command = Command::new("cargo");
    command
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .current_dir(top);
    for name in git_local_env_vars()? {
        command.env_remove(name);
    }
    let output = command
        .output()
        .context(|| "running cargo metadata".to_string())?;
    if !output.status.success() {
        fail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context(|| "reading cargo metadata".to_string())?;
    let root = std::fs::canonicalize(top).context(|| format!("resolving {}", top.display()))?;
    let mut crates = Vec::new();
    for package in json["packages"].as_array().into_iter().flatten() {
        let (Some(name), Some(manifest)) =
            (package["name"].as_str(), package["manifest_path"].as_str())
        else {
            fail!("cargo metadata has a package without a name or manifest path");
        };
        let Some(dir) = PathBuf::from(manifest)
            .parent()
            .and_then(|dir| std::fs::canonicalize(dir).ok())
        else {
            continue;
        };
        let Ok(relative) = dir.strip_prefix(&root) else {
            continue;
        };
        crates.push(Crate {
            name: name.to_string(),
            dir: relative.to_string_lossy().into_owned(),
        });
    }
    Ok(crates)
}

/// The root lock file.
pub(crate) const LOCK: &str = "Cargo.lock";

/// The crates whose resolved dependencies changed in the root `Cargo.lock`
/// since <base>, or every crate when there is no <base> or <base> had no
/// lock.
fn lock_crates(top: &Path, crates: &[Crate], base: Option<&str>) -> Result<Vec<String>> {
    let every = || crates.iter().map(|c| c.name.clone()).collect();
    let Some(base) = base else {
        return Ok(every());
    };
    let Some(old) = crate::lock_diff::lock_at(top, base)? else {
        return Ok(every());
    };
    let path = top.join(LOCK);
    let new = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    let names: Vec<String> = crates.iter().map(|c| c.name.clone()).collect();
    crate::lock_diff::with_changed_dependencies(&old, &new, &names)
}

/// Root files that configure every crate: the workspace manifest, with its
/// lints and patches, clippy's configuration, the toolchain rustup picks,
/// which compiles every crate and holds the clippy that lints them, and
/// cargo's configuration.
pub(crate) const ROOT_CONFIGS: [&str; 7] = [
    "Cargo.toml",
    "clippy.toml",
    ".clippy.toml",
    "rust-toolchain.toml",
    "rust-toolchain",
    ".cargo/config.toml",
    ".cargo/config",
];

/// The names of the crates that hold some of <files> (the deepest crate whose
/// folder contains one), sorted. A root file of `ROOT_CONFIGS`, and a file
/// <line> marks `every:`, counts as every crate, and the root `Cargo.lock` as
/// the crates <lock> names (`lock_crates`).
fn crates_holding(line: &Line, crates: &[Crate], files: &[String], lock: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for file in files {
        if ROOT_CONFIGS.contains(&file.as_str()) || line.counts_as_every_crate(file) {
            names.extend(crates.iter().map(|c| c.name.clone()));
            continue;
        }
        if file == LOCK {
            names.extend(lock.iter().cloned());
            continue;
        }
        let holder = crates
            .iter()
            .filter(|c| {
                c.dir.is_empty()
                    || file
                        .strip_prefix(c.dir.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|c| c.dir.len());
        if let Some(holder) = holder {
            names.push(holder.name.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
#[path = "checks_tests.rs"]
mod tests;
