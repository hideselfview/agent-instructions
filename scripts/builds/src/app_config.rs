//! A repo's `.app-build`: what only the repo knows about building its apps in
//! its app checkout, one field per line. `builds build-app` (build_app.rs)
//! reads a platform's lines, `builds app-build` (app_build.rs) the `step`
//! lines:
//!
//! ```text
//! mac build ./bae-macos/run.sh --no-open
//! mac app bae-macos/bae/.build/runApp/Build/Products/Debug/bae.app
//! mac bundle fm.bae.desktop
//! android build ./bae-android/run.sh
//! step macos ./bae-macos/run.sh --no-open
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lock;
use crate::places::Places;

pub const CONFIG: &str = ".app-build";

/// A repo's `.app-build`.
#[derive(Debug, PartialEq, Eq)]
pub struct Config {
    pub mac: Option<MacApp>,
    pub android: Option<AndroidApp>,
    /// What `builds app-build` runs, in the order written.
    pub steps: Vec<Step>,
}

/// The `mac` lines.
#[derive(Debug, PartialEq, Eq)]
pub struct MacApp {
    /// The build's program, relative to the checkout when it names a path,
    /// and its arguments.
    pub build: Vec<String>,
    /// The built app, relative to the checkout.
    pub app: String,
    /// The app's bundle id, to quit the one running.
    pub bundle: String,
}

/// The `android` line.
#[derive(Debug, PartialEq, Eq)]
pub struct AndroidApp {
    /// The build's program, as for `MacApp::build`, which also installs the
    /// app wherever it should go.
    pub build: Vec<String>,
}

/// A `step <name> <program> [<args>...]` line: a program, relative to the
/// checkout when it names a path. No shell reads it: a step that needs one is
/// a script in the repo.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Step {
    pub name: String,
    pub command: Vec<String>,
}

fn words(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_string).collect()
}

pub fn parse_config(text: &str) -> Result<Config> {
    let (mut mac_build, mut mac_app, mut bundle) = (None, None, None);
    let mut android = None;
    let mut steps: Vec<Step> = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (platform, field) = line.split_once(' ').unwrap_or((line, ""));
        let (name, value) = field.trim().split_once(' ').unwrap_or((field.trim(), ""));
        let value = value.trim();
        match (platform, name) {
            ("mac", "build") => mac_build = Some(words(value)),
            ("mac", "app") => mac_app = Some(value.to_string()),
            ("mac", "bundle") => bundle = Some(value.to_string()),
            ("android", "build") => android = Some(AndroidApp { build: words(value) }),
            ("step", _) => {
                let mut words = field.split_whitespace().map(str::to_string);
                let (Some(name), command) = (words.next(), words.collect::<Vec<_>>()) else {
                    fail!("{CONFIG}: a step needs a name and a command");
                };
                if command.is_empty() {
                    fail!("{CONFIG}: step {name} names no command");
                }
                if steps.iter().any(|step| step.name == name) {
                    fail!("{CONFIG}: step {name} is named twice");
                }
                steps.push(Step { name, command });
            }
            _ => fail!("{CONFIG}: no field in {line:?}"),
        }
    }
    let mac = match (mac_build, mac_app, bundle) {
        (Some(build), Some(app), Some(bundle)) => Some(MacApp { build, app, bundle }),
        (None, None, None) => None,
        _ => fail!("{CONFIG}: mac needs build, app and bundle lines"),
    };
    Ok(Config { mac, android, steps })
}

/// Runs git in <top>, failing with what it said.
pub fn run_git(top: &Path, args: &[&str]) -> Result<String> {
    git_text(git(top).args(args), &format!("git {}", args.join(" ")))
}

/// The ref checked out in the app checkout <top>, which must be clean, and its
/// `.app-build`.
pub fn check_out(top: &Path, commit: &str) -> Result<Config> {
    let changes = run_git(top, &["status", "--porcelain"])?;
    if !changes.trim().is_empty() {
        fail!("the app checkout has changes, so nothing was built:\n{changes}");
    }
    run_git(top, &["fetch", "-q", "origin"])?;
    run_git(top, &["checkout", "-q", "--detach", commit])?;
    let file = top.join(CONFIG);
    parse_config(&std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?)
}

/// Runs <command> in the checkout <top>, in a process group of its own.
pub fn run_command(top: &Path, command: &[String]) -> Result<lock::Ran> {
    let Some((program, args)) = command.split_first() else {
        fail!("{CONFIG}: a command names no program");
    };
    let path = if program.contains('/') { top.join(program) } else { PathBuf::from(program) };
    let mut start = Command::new(path);
    start.args(args).current_dir(top);
    lock::run_in_group(start, program)
}

/// The folder of <repo>'s app checkout.
pub fn app_checkout_top(places: &Places, repo: &str) -> Result<PathBuf> {
    let Some(name) = places.app_checkout_of(repo)? else {
        fail!("{repo} has no app checkout; builds app-checkout records one");
    };
    let marker = places.root.join(repo).join(&name).join(".checkout");
    let text = std::fs::read_to_string(&marker).context(|| format!("reading {}", marker.display()))?;
    Ok(PathBuf::from(text.trim()))
}

#[cfg(test)]
#[path = "app_config_tests.rs"]
mod tests;
