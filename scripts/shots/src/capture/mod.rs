//! Running a platform's harness and collecting the shots it writes.
//!
//! The app's `shots-capture <platform> <command>` line names the command that
//! takes the platform's shots. builds-shots runs it from the repo's top with
//! these variables, and the command passes them to its harness:
//!
//! - `BUILDS_SHOTS_OUT`: the absolute folder the harness writes each shot to,
//!   as `<name>@<platform>.png` and `<name>@<platform>.json`.
//! - `BUILDS_SHOTS_SELECT`: the name prefixes of the shots to take, comma
//!   separated; unset for every shot of the platform.
//! - `BUILDS_SHOTS_WINDOWS`: `1` when the harness also takes the shots shown
//!   in a window, which bring the app to the front; unset otherwise.
//!
//! On mac each variable is also set with `TEST_RUNNER_` in front, which Xcode
//! passes to the test process under the plain name.

use crate::declared::Declared;
use crate::names::{Name, Platform, Prefix};
use crate::shots::Shot;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CaptureError {
    #[error("cannot start {step}: {source}")]
    Start {
        step: String,
        source: std::io::Error,
    },
    #[error("{step} failed: {status}")]
    Failed {
        step: String,
        status: std::process::ExitStatus,
    },
    #[error("the repo's .app-build has no shots-capture line for {0}")]
    NoCommand(&'static str),
    #[error("cannot prepare {0}: {1}")]
    Directory(PathBuf, std::io::Error),
    #[error("the prefix '{0}' selects no declared shot")]
    UnknownPrefix(String),
    #[error("no declared {0:?} shot is selected")]
    NothingSelected(Platform),
    #[error("the capture is incomplete:\n{}", .0.join("\n"))]
    Incomplete(Vec<String>),
}

/// The declared shots of `platform` a run takes: all of them when no prefix is
/// given, else those a prefix of that platform starts.
pub(crate) fn selected(
    declared: &[Name],
    platform: Platform,
    prefixes: &[Prefix],
) -> Result<Vec<Name>, CaptureError> {
    for prefix in prefixes {
        if !declared.iter().any(|name| prefix.matches(name)) {
            return Err(CaptureError::UnknownPrefix(prefix.as_str().to_string()));
        }
    }
    let selected: Vec<Name> = declared
        .iter()
        .filter(|name| name.platform() == platform)
        .filter(|name| prefixes.is_empty() || prefixes.iter().any(|p| p.matches(name)))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err(CaptureError::NothingSelected(platform));
    }
    Ok(selected)
}

/// Why a shot is left out of a run given no `--windows`.
pub(crate) const NEEDS_WINDOW: &str = "not taken here (needs a window)";

/// The shots of `platform` that `prefixes` select, split into those this run
/// takes and those it leaves out because they are shown in a window and the
/// run was not given `--windows`.
#[derive(Debug)]
pub(crate) struct Taken {
    pub(crate) names: Vec<Name>,
    pub(crate) not_taken: Vec<Name>,
}

pub(crate) fn taken(
    declared: &[Declared],
    platform: Platform,
    prefixes: &[Prefix],
    windows: bool,
) -> Result<Taken, CaptureError> {
    let all: Vec<Name> = declared.iter().map(|d| d.name.clone()).collect();
    let (names, not_taken) = selected(&all, platform, prefixes)?
        .into_iter()
        .partition(|name| windows || !declared.iter().any(|d| &d.name == name && d.needs_window));
    Ok(Taken { names, not_taken })
}

const EXTENSIONS: [&str; 2] = ["png", "json"];

/// Empties `out` of `platform`'s shot files, so a capture starts from nothing
/// and the completeness check sees only what this run wrote. The folder is
/// shared by every branch built in a checkout; what another branch left behind
/// would otherwise read as undeclared.
pub(crate) fn clear(out: &Path, platform: Platform) -> Result<(), CaptureError> {
    std::fs::create_dir_all(out).map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
    let entries =
        std::fs::read_dir(out).map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if Name::from_file_name(platform, &file).is_some() {
            std::fs::remove_file(entry.path())
                .map_err(|e| CaptureError::Directory(entry.path(), e))?;
        }
    }
    Ok(())
}

/// Takes the shots of `platform` that `prefixes` select (all when empty) into
/// `out`, an absolute path, with the `command` the app's `shots-capture` line
/// gives, run from `root`. Without `windows`, a shot shown in its window is
/// left out and said on stderr. Returns the shots taken and those left out.
pub(crate) fn capture(
    root: &Path,
    command: &[String],
    declared: &[Declared],
    platform: Platform,
    prefixes: &[Prefix],
    windows: bool,
    out: &Path,
) -> Result<Taken, CaptureError> {
    let taken = taken(declared, platform, prefixes, windows)?;
    for name in &taken.not_taken {
        eprintln!("builds shots: {name}: {NEEDS_WINDOW}");
    }
    clear(out, platform)?;
    if taken.names.is_empty() {
        return Ok(taken);
    }
    let step = format!("the {} shots-capture command", platform.word());
    let (program, args) = command
        .split_first()
        .ok_or(CaptureError::NoCommand(platform.word()))?;
    eprintln!("builds shots: {}", command.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .envs(variables(
            platform,
            out,
            &harness_prefixes(platform, prefixes, &taken.names),
            windows,
        ))
        .status()
        .map_err(|source| CaptureError::Start {
            step: step.clone(),
            source,
        })?;
    if !status.success() {
        return Err(CaptureError::Failed { step, status });
    }
    let declared_here: Vec<Name> = declared
        .iter()
        .map(|d| d.name.clone())
        .filter(|n| n.platform() == platform)
        .collect();
    check_complete(out, platform, &taken.names, &declared_here)?;
    Ok(taken)
}

/// The prefixes the harness is given: those of `platform` that start a shot
/// this run takes. A harness may fail on a prefix that starts none of the
/// shots it draws, and without `--windows` it draws no window's shot.
fn harness_prefixes(platform: Platform, prefixes: &[Prefix], taken: &[Name]) -> Vec<String> {
    prefixes
        .iter()
        .filter(|p| p.platform() == platform && taken.iter().any(|n| p.matches(n)))
        .map(|p| p.as_str().to_string())
        .collect()
}

/// The variables the capture command runs with (the module's doc).
fn variables(
    platform: Platform,
    out: &Path,
    prefixes: &[String],
    windows: bool,
) -> Vec<(String, String)> {
    let mut plain = vec![("BUILDS_SHOTS_OUT", out.to_string_lossy().into_owned())];
    if !prefixes.is_empty() {
        plain.push(("BUILDS_SHOTS_SELECT", prefixes.join(",")));
    }
    if windows {
        plain.push(("BUILDS_SHOTS_WINDOWS", "1".to_string()));
    }
    let mut all: Vec<(String, String)> = plain
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    if platform == Platform::Mac {
        all.extend(
            plain
                .iter()
                .map(|(k, v)| (format!("TEST_RUNNER_{k}"), v.clone())),
        );
    }
    all
}

/// Compares what a capture wrote with what it was to write, both ways: every
/// selected shot wrote its PNG and its element list, which read back and agree
/// with each other, and `out` holds no shot file of `platform` that no harness
/// declares.
pub(crate) fn check_complete(
    out: &Path,
    platform: Platform,
    selected: &[Name],
    declared: &[Name],
) -> Result<(), CaptureError> {
    let mut problems = Vec::new();
    for name in selected {
        let mut written = true;
        for extension in EXTENSIONS {
            let file = name.file_name(extension);
            if !out.join(&file).is_file() {
                problems.push(format!("missing: the harness wrote no {file}"));
                written = false;
            }
        }
        if !written {
            continue;
        }
        match Shot::load(out, name) {
            Ok(shot) => problems.extend(
                shot.problems(name)
                    .into_iter()
                    .map(|problem| format!("inconsistent: {name}: {problem}")),
            ),
            Err(e) => problems.push(format!("unreadable: {name}: {e}")),
        }
    }
    let entries =
        std::fs::read_dir(out).map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
        let file = entry.file_name().to_string_lossy().into_owned();
        match Name::from_file_name(platform, &file) {
            None => {}
            Some(Ok(name)) if declared.contains(&name) => {}
            Some(_) => problems.push(format!("undeclared: {file} is no declared shot")),
        }
    }
    problems.sort();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(CaptureError::Incomplete(problems))
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
