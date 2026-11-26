//! Running an app's harness and collecting the shots it writes.
//!
//! The app's `shots-capture <app> <command>` line names the command that
//! takes the app's shots. The checker runs it from the repo's top with
//! these variables, and the command passes them to its harness:
//!
//! - `BUILDS_SHOTS_OUT`: the absolute folder the harness writes each shot to,
//!   as `<name>@<file word>.png` and `<name>@<file word>.json`, the file
//!   word being the platform's.
//! - `BUILDS_SHOTS_SELECT`: the name prefixes of the shots to take, comma
//!   separated; unset for every shot of the app.
//! - `BUILDS_SHOTS_BOARD_SIZES`: the absolute path of `board-sizes.json` in
//!   the out folder: one JSON object that maps the name of each selected
//!   component scene to `{"width": <points>, "height": <points>}`, the size of
//!   the scene's board. It is `{}` when no component scene is selected. The
//!   checker writes the whole file before it starts the command.
//! - `BUILDS_SHOTS_WINDOWS`: `1` when the harness also takes the shots that
//!   take the keyboard focus, which bring the app to the front; unset
//!   otherwise.
//! - `BUILDS_SHOTS_DISPLAY`: on mac, the absolute path of the tool that leases
//!   the shot display for the shots shown in a window, which the harness the
//!   checker holds runs (harness.rs). The checker writes the tool before it
//!   starts the command and removes it after.
//!
//! On mac each variable is also set with `TEST_RUNNER_` in front, which Xcode
//! passes to the test process under the plain name.
//!
//! A web app has no harness: its `shots-stage` line names the command that
//! serves each shot's page, and the checker takes the shot itself (web.rs).

pub(crate) mod web;

use crate::backend;
use crate::declared::Declared;
use crate::names::{App, Apps, Name, Platform, Prefix};
use crate::shots::{Shot, Size};
use std::collections::BTreeMap;
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
    #[error("the repo's .app-build has no shots-capture or shots-stage line for the app {0}")]
    NoCommand(String),
    #[error("cannot prepare {0}: {1}")]
    Directory(PathBuf, std::io::Error),
    #[error("the prefix '{0}' selects no declared shot")]
    UnknownPrefix(String),
    #[error("no declared {0} shot is selected")]
    NothingSelected(String),
    #[error("cannot measure the boards: {0}")]
    Measure(String),
    #[error("cannot write {0}: {1}")]
    BoardSizes(PathBuf, std::io::Error),
    #[error(transparent)]
    Harness(#[from] crate::harness::HarnessError),
    #[error("cannot take shots in Chrome: {0}")]
    Chrome(String),
    #[error("the capture is incomplete:\n{}", .0.join("\n"))]
    Incomplete(Vec<String>),
}

/// The declared shots of `app` a run takes: all of them when no prefix is
/// given, else those a prefix of that app starts. A prefix that starts no
/// declared shot fails the run, except in the run of a merge base (<base>),
/// where the branch may add the shots the prefix names: there it selects
/// none, and the run has no pairs.
pub(crate) fn selected(
    declared: &[Name],
    app: &App,
    prefixes: &[Prefix],
    base: bool,
) -> Result<Vec<Name>, CaptureError> {
    for prefix in prefixes {
        if !declared.iter().any(|name| prefix.matches(name)) {
            if !base {
                return Err(CaptureError::UnknownPrefix(prefix.as_str().to_string()));
            }
            crate::log::note(format_args!(
                "the prefix '{}' starts no declared shot at the merge base, so the run takes none for it",
                prefix.as_str()
            ));
        }
    }
    let selected: Vec<Name> = declared
        .iter()
        .filter(|name| name.app() == app)
        .filter(|name| prefixes.is_empty() || prefixes.iter().any(|p| p.matches(name)))
        .cloned()
        .collect();
    if selected.is_empty() {
        if !base {
            return Err(CaptureError::NothingSelected(app.name.clone()));
        }
        crate::log::note(format_args!(
            "no declared {} shot is selected at the merge base, so the run has no pairs",
            app.name
        ));
    }
    Ok(selected)
}

/// Why a shot is left out of a run given no `--windows`.
pub(crate) const TAKES_FOCUS: &str = "not taken here (takes the keyboard focus)";

/// The shots of `app` that `prefixes` select, split into those this run
/// takes and those it leaves out because they take the keyboard focus and the
/// run was not given `--windows`.
#[derive(Debug)]
pub(crate) struct Taken {
    /// The shots the run took: for a capture, those that wrote a usable shot.
    pub(crate) names: Vec<Name>,
    pub(crate) not_taken: Vec<Name>,
    /// The shots the harness was to write and wrote no usable shot of, with
    /// why; each fails the run, and the run still checks `names`.
    pub(crate) failed: Vec<Failed>,
}

/// Prints a line for each of the shots <failed>.
pub(crate) fn print_failed(failed: &[Failed]) {
    for failed in failed {
        crate::log::outcome(format_args!("FAIL {}: {}", failed.name, failed.why));
    }
}

/// A selected shot a capture wrote no usable files of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failed {
    pub(crate) name: Name,
    pub(crate) why: String,
}

pub(crate) fn taken(
    declared: &[Declared],
    app: &App,
    prefixes: &[Prefix],
    windows: bool,
    base: bool,
) -> Result<Taken, CaptureError> {
    let all: Vec<Name> = declared.iter().map(|d| d.name.clone()).collect();
    let (names, not_taken) = selected(&all, app, prefixes, base)?
        .into_iter()
        .partition(|name| {
            windows
                || !declared
                    .iter()
                    .any(|d| &d.name == name && d.marks.takes_focus)
        });
    Ok(Taken {
        names,
        not_taken,
        failed: Vec::new(),
    })
}

const EXTENSIONS: [&str; 2] = ["png", "json"];

/// Empties `out` of `app`'s shot files, so a capture starts from nothing
/// and the completeness check sees only what this run wrote. The folder is
/// shared by every branch built in a checkout; what another branch left behind
/// would otherwise read as undeclared.
pub(crate) fn clear(out: &Path, app: &App, apps: &Apps) -> Result<(), CaptureError> {
    std::fs::create_dir_all(out).map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
    let entries =
        std::fs::read_dir(out).map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if Name::from_file_name(app, apps, &file).is_some() {
            std::fs::remove_file(entry.path())
                .map_err(|e| CaptureError::Directory(entry.path(), e))?;
        }
    }
    Ok(())
}

/// The board size of each of the shots, in points.
pub(crate) type Sizes = BTreeMap<Name, Size>;

/// Where a capture writes and how it learns the boards' sizes.
pub(crate) struct Target<'a> {
    /// The absolute folder the harness writes the shots to.
    pub(crate) out: &'a Path,
    /// Gives the board size of each of the shots a run takes.
    pub(crate) measure: &'a dyn Fn(&[Name]) -> Result<Sizes, String>,
}

/// Takes the shots of `app` that `prefixes` select (all when empty) into
/// `target.out`, an absolute path, with the `command` the app's `shots-capture` line
/// gives, run from `root`; with shots to take and no `command`, it fails.
/// Without `windows`, a shot that takes the keyboard focus is left out and
/// said on stderr. Before the command starts, the run writes the host tools
/// that the platform's harness has, and `target.measure` gives the board size of each
/// shot the run takes, which the run writes to `board-sizes.json` in
/// `target.out` (the module's doc). Returns the shots taken, those the
/// harness wrote no usable shot of (a failed shot does not fail the capture
/// itself) and those left out.
#[allow(clippy::too_many_arguments)]
pub(crate) fn capture(
    root: &Path,
    command: Option<&[String]>,
    declared: &[Declared],
    app: &App,
    apps: &Apps,
    prefixes: &[Prefix],
    windows: bool,
    base: bool,
    target: &Target,
) -> Result<Taken, CaptureError> {
    let Target { out, measure } = *target;
    let taken = taken(declared, app, prefixes, windows, base)?;
    for name in &taken.not_taken {
        crate::log::note(format_args!("{name}: {TAKES_FOCUS}"));
    }
    clear(out, app, apps)?;
    if taken.names.is_empty() {
        return Ok(taken);
    }
    let sizes = write_board_sizes(out, &measure(&taken.names).map_err(CaptureError::Measure)?)?;
    let step = format!("the {} shots-capture command", app.name);
    let command = command.ok_or_else(|| CaptureError::NoCommand(app.name.clone()))?;
    let (program, args) = command.split_first().expect("the command has a program");
    let display = backend::of(app.platform)
        .harness()
        .and_then(|harness| harness.display)
        .map(crate::harness::write_display_tool)
        .transpose()?;
    let tool = display
        .as_ref()
        .map(|folder| folder.path().join(crate::harness::DISPLAY_TOOL));
    crate::log::note(command.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .envs(variables(
            app.platform,
            out,
            &sizes,
            &harness_prefixes(app, prefixes, &taken.names),
            windows,
            tool.as_deref(),
        ))
        .status()
        .map_err(|source| CaptureError::Start {
            step: step.clone(),
            source,
        })?;
    let declared_here: Vec<Name> = declared
        .iter()
        .map(|d| d.name.clone())
        .filter(|n| n.app() == app)
        .collect();
    let stray = undeclared_problems(out, app, apps, &declared_here)?;
    if !stray.is_empty() {
        return Err(CaptureError::Incomplete(stray));
    }
    let mut failed = Vec::new();
    let mut names = Vec::new();
    for name in taken.names {
        let problems = scene_problems(out, &name);
        if problems.is_empty() {
            names.push(name);
        } else {
            let why = if status.success() {
                problems.join("; ")
            } else {
                format!("{step} failed ({status}): {}", problems.join("; "))
            };
            failed.push(Failed { name, why });
        }
    }
    if failed.is_empty() && !status.success() {
        return Err(CaptureError::Failed { step, status });
    }
    Ok(Taken {
        names,
        not_taken: taken.not_taken,
        failed,
    })
}

/// The prefixes the harness is given: those of `app` that start a shot
/// this run takes. A harness may fail on a prefix that starts none of the
/// shots it draws, and without `--windows` it draws no shot that takes the
/// focus.
fn harness_prefixes(app: &App, prefixes: &[Prefix], taken: &[Name]) -> Vec<String> {
    prefixes
        .iter()
        .filter(|p| p.app() == app.name && taken.iter().any(|n| p.matches(n)))
        .map(|p| p.as_str().to_string())
        .collect()
}

const BOARD_SIZES_FILE: &str = "board-sizes.json";

/// Writes `board-sizes.json` into `out` whole: to a temporary file in `out`,
/// then renamed over the name, so a reader never sees half of it. Returns its
/// path.
fn write_board_sizes(out: &Path, sizes: &BTreeMap<Name, Size>) -> Result<PathBuf, CaptureError> {
    let by_name: BTreeMap<String, &Size> = sizes.iter().map(|(n, s)| (n.to_string(), s)).collect();
    let file = out.join(BOARD_SIZES_FILE);
    let temporary = out.join(format!(".{BOARD_SIZES_FILE}.tmp"));
    let fail = |path: &Path| {
        let path = path.to_path_buf();
        move |e: std::io::Error| CaptureError::BoardSizes(path, e)
    };
    let text = serde_json::to_vec_pretty(&by_name).expect("sizes serialize");
    std::fs::write(&temporary, text).map_err(fail(&temporary))?;
    std::fs::rename(&temporary, &file).map_err(fail(&file))?;
    Ok(file)
}

/// The variables the capture command runs with (the module's doc).
fn variables(
    platform: Platform,
    out: &Path,
    sizes: &Path,
    prefixes: &[String],
    windows: bool,
    display_tool: Option<&Path>,
) -> Vec<(String, String)> {
    let mut plain = vec![
        ("BUILDS_SHOTS_OUT", out.to_string_lossy().into_owned()),
        (
            "BUILDS_SHOTS_BOARD_SIZES",
            sizes.to_string_lossy().into_owned(),
        ),
    ];
    if !prefixes.is_empty() {
        plain.push(("BUILDS_SHOTS_SELECT", prefixes.join(",")));
    }
    if windows {
        plain.push(("BUILDS_SHOTS_WINDOWS", "1".to_string()));
    }
    if let Some(tool) = display_tool {
        plain.push(("BUILDS_SHOTS_DISPLAY", tool.to_string_lossy().into_owned()));
    }
    let mut all: Vec<(String, String)> = plain
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    if backend::of(platform).test_runner_copies() {
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
/// with each other, and `out` holds no shot file of `app` that no harness
/// declares.
pub(crate) fn check_complete(
    out: &Path,
    app: &App,
    apps: &Apps,
    selected: &[Name],
    declared: &[Name],
) -> Result<(), CaptureError> {
    let mut problems: Vec<String> = selected
        .iter()
        .flat_map(|name| scene_problems(out, name))
        .collect();
    problems.extend(undeclared_problems(out, app, apps, declared)?);
    problems.sort();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(CaptureError::Incomplete(problems))
    }
}

/// What is wrong with the files `name`'s shot wrote into `out`: a file
/// missing, or files that do not read back or disagree.
fn scene_problems(out: &Path, name: &Name) -> Vec<String> {
    let mut problems = Vec::new();
    for extension in EXTENSIONS {
        let file = name.file_name(extension);
        if !out.join(&file).is_file() {
            problems.push(format!("missing: the harness wrote no {file}"));
        }
    }
    if !problems.is_empty() {
        return problems;
    }
    match Shot::load(out, name) {
        Ok(shot) => problems.extend(
            shot.problems(name)
                .into_iter()
                .map(|problem| format!("inconsistent: {name}: {problem}")),
        ),
        Err(e) => problems.push(format!("unreadable: {name}: {e}")),
    }
    problems
}

/// The shot files of `app` in `out` that no harness declares.
fn undeclared_problems(
    out: &Path,
    app: &App,
    apps: &Apps,
    declared: &[Name],
) -> Result<Vec<String>, CaptureError> {
    let mut problems = Vec::new();
    let entries =
        std::fs::read_dir(out).map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| CaptureError::Directory(out.to_path_buf(), e))?;
        let file = entry.file_name().to_string_lossy().into_owned();
        match Name::from_file_name(app, apps, &file) {
            None => {}
            Some(Ok(name)) if declared.contains(&name) => {}
            Some(_) => problems.push(format!("undeclared: {file} is no declared shot")),
        }
    }
    problems.sort();
    Ok(problems)
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
