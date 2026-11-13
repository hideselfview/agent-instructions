//! A repo's `.checks`: the commit checks the repo declares, one per line,
//! the way `.app-build` declares app build steps:
//!
//! ```text
//! check fmt *.rs cargo fmt --check
//! fix swift-format *.swift swift-format format --in-place {files}
//! check clippy *.rs,Cargo.toml,Cargo.lock cargo clippy {crates} -- -D warnings
//! test loc-words scripts/loc-words.py,spec/words.md scripts/loc-words-test.sh
//! ```
//!
//! `<kind> <name> <paths> <program> [<args>...]`. The program is not run by a
//! shell. `checks_run.rs` picks the lines a set of changed files matches and
//! runs them; this file reads the lines and expands what a match hands the
//! program.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::checkout::git_local_env_vars;
use crate::fail;
use crate::fail::{Context, Result};

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
    /// Globs over repo-relative paths.
    pub paths: Vec<String>,
    /// The program, relative to the repo when it names a path, and its
    /// arguments, `{files}` and `{crates}` not yet expanded.
    pub command: Vec<String>,
}

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
    for raw in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let mut words = raw.split_whitespace().map(str::to_string);
        let (Some(kind), Some(name), Some(paths)) = (words.next(), words.next(), words.next()) else {
            fail!("{CONFIG}: {raw:?} needs a kind, a name, paths and a command");
        };
        let kind = match kind.as_str() {
            "check" => Kind::Check,
            "fix" => Kind::Fix,
            "test" => Kind::Test,
            other => fail!("{CONFIG}: {raw:?} starts with {other:?}, not check, fix or test"),
        };
        let command: Vec<String> = words.collect();
        if command.is_empty() {
            fail!("{CONFIG}: {} {name} names no command", kind.word());
        }
        if lines.iter().any(|line| line.name == name) {
            fail!("{CONFIG}: {name} is named twice");
        }
        let paths: Vec<String> = paths.split(',').filter(|p| !p.is_empty()).map(str::to_string).collect();
        if paths.is_empty() {
            fail!("{CONFIG}: {name} names no paths");
        }
        lines.push(Line { kind, name, paths, command });
    }
    Ok(lines)
}

/// Whether <path> (repo-relative, `/`-separated) matches <pattern>: `*` stays
/// inside one path segment, a `**` segment is any number of segments, and a
/// pattern with no `/` is matched against the file name wherever it sits.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let path: Vec<&str> = path.split('/').collect();
    if !pattern.contains('/') {
        return path.last().is_some_and(|name| segment_match(pattern, name));
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    segments_match(&pattern, &path)
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => match path.split_first() {
            Some((name, path_rest)) => segment_match(first, name) && segments_match(rest, path_rest),
            None => false,
        },
    }
}

fn segment_match(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => {
            let Some(after_head) = name.strip_prefix(head) else { return false };
            (0..=after_head.len()).any(|skip| after_head.is_char_boundary(skip) && segment_match(tail, &after_head[skip..]))
        }
    }
}

impl Line {
    pub fn matches(&self, path: &str) -> bool {
        self.paths.iter().any(|glob| glob_match(glob, path))
    }

    fn uses(&self, placeholder: &str) -> bool {
        self.command.iter().any(|arg| arg == placeholder)
    }
}

/// The lines of <kinds> that some of <files> match, each with its command
/// expanded, in file order. A line that names `{files}` or `{crates}` and has
/// nothing to hand it (no matching file still there, no crate holding one) is
/// left out. With <every>, a line is picked whether or not a file matches it.
pub fn plan(top: &Path, lines: &[Line], kinds: &[Kind], files: &[String], every: bool) -> Result<Vec<Planned>> {
    let mut metadata: Option<Vec<Crate>> = None;
    let mut planned = Vec::new();
    for line in lines.iter().filter(|line| kinds.contains(&line.kind)) {
        let matching: Vec<String> = files.iter().filter(|file| line.matches(file)).cloned().collect();
        if matching.is_empty() && !every {
            continue;
        }
        let existing: Vec<String> = matching.into_iter().filter(|file| top.join(file).exists()).collect();
        let crates = if line.uses("{crates}") {
            if metadata.is_none() {
                metadata = Some(workspace_crates(top)?);
            }
            let held = crates_holding(metadata.as_deref().expect("just read"), &existing);
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
                "{crates}" => argv.extend(crates.iter().flat_map(|name| ["-p".to_string(), name.clone()])),
                _ => argv.push(arg.clone()),
            }
        }
        planned.push(Planned { line: line.clone(), files: existing, argv });
    }
    Ok(planned)
}

struct Crate {
    name: String,
    /// The crate's folder relative to the repo, empty for the repo itself.
    dir: String,
}

fn workspace_crates(top: &Path) -> Result<Vec<Crate>> {
    let mut command = Command::new("cargo");
    command.args(["metadata", "--no-deps", "--offline", "--format-version", "1"]).current_dir(top);
    for name in git_local_env_vars()? {
        command.env_remove(name);
    }
    let output = command.output().context(|| "running cargo metadata".to_string())?;
    if !output.status.success() {
        fail!("cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context(|| "reading cargo metadata".to_string())?;
    let root = std::fs::canonicalize(top).context(|| format!("resolving {}", top.display()))?;
    let mut crates = Vec::new();
    for package in json["packages"].as_array().into_iter().flatten() {
        let (Some(name), Some(manifest)) = (package["name"].as_str(), package["manifest_path"].as_str()) else {
            fail!("cargo metadata has a package without a name or manifest path");
        };
        let Some(dir) = PathBuf::from(manifest).parent().and_then(|dir| std::fs::canonicalize(dir).ok()) else {
            continue;
        };
        let Ok(relative) = dir.strip_prefix(&root) else { continue };
        crates.push(Crate { name: name.to_string(), dir: relative.to_string_lossy().into_owned() });
    }
    Ok(crates)
}

/// The names of the crates that hold some of <files> (the deepest crate whose
/// folder contains one), sorted. A root `Cargo.toml` or `Cargo.lock` counts as
/// every crate.
fn crates_holding(crates: &[Crate], files: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for file in files {
        if file == "Cargo.toml" || file == "Cargo.lock" {
            names.extend(crates.iter().map(|c| c.name.clone()));
            continue;
        }
        let holder = crates
            .iter()
            .filter(|c| c.dir.is_empty() || file.strip_prefix(c.dir.as_str()).is_some_and(|rest| rest.starts_with('/')))
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
