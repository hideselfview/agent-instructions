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
//! step mac macos ./bae-macos/run.sh --no-open
//! step android android ./bae-android/compile-all.sh
//! limit macos 90m
//! ```
//!
//! A step belongs to a lane (`mac` or `android`, lane.rs), named first: the
//! lane's app checkout and lock are where it runs.
//!
//! A step runs for at most `DEFAULT_STEP_LIMIT`, or the time a `limit <step>
//! <duration>` line gives it (`90s`, `30m`, `2h`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::lock::{self, Within};
use crate::places::Places;

pub const CONFIG: &str = ".app-build";

/// How long a step may run unless its `limit` line says otherwise. Past it the
/// step is killed and the build fails, so the lock goes on to the next in
/// line. A hung macOS test once held the lock for 33 minutes with five builds
/// waiting. Steps measured on this machine ran well under ten minutes warm
/// (an Xcode build, a Gradle compile); a cold bridge build for a platform,
/// not measured, is the longest a step legitimately runs, and this leaves it
/// room.
pub const DEFAULT_STEP_LIMIT: Duration = Duration::from_secs(45 * 60);

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

/// A `step <lane> <name> <program> [<args>...]` line: a program, relative to
/// the checkout when it names a path. No shell reads it: a step that needs one is
/// a script in the repo.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Step {
    pub name: String,
    pub lane: Lane,
    pub command: Vec<String>,
    /// From the step's `limit` line; `DEFAULT_STEP_LIMIT` without one.
    pub limit: Option<Duration>,
}

impl Step {
    pub fn limit(&self) -> Duration {
        self.limit.unwrap_or(DEFAULT_STEP_LIMIT)
    }
}

/// <text> (`90s`, `30m`, `2h`) as a time.
fn parse_duration(text: &str) -> Option<Duration> {
    let (digits, unit) = text.split_at(text.len().saturating_sub(1));
    let n: u64 = digits.parse().ok().filter(|n| *n > 0)?;
    let seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return None,
    };
    Some(Duration::from_secs(n * seconds))
}

/// <limit> as `.app-build` writes it and a message should: whole hours,
/// minutes or seconds.
pub fn show_duration(limit: Duration) -> String {
    let seconds = limit.as_secs();
    match seconds {
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn words(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_string).collect()
}

pub fn parse_config(text: &str) -> Result<Config> {
    let (mut mac_build, mut mac_app, mut bundle) = (None, None, None);
    let mut android = None;
    let mut steps: Vec<Step> = Vec::new();
    let mut limits: Vec<(String, Duration)> = Vec::new();
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
                let (Some(lane), Some(name)) = (words.next(), words.next()) else {
                    fail!("{CONFIG}: a step needs a lane, a name and a command");
                };
                let command = words.collect::<Vec<_>>();
                let Some(lane) = Lane::parse(&lane) else {
                    fail!("{CONFIG}: {line:?}: a step starts with its lane, mac or android, not {lane:?}");
                };
                if command.is_empty() {
                    fail!("{CONFIG}: step {name} names no command");
                }
                if steps.iter().any(|step| step.name == name) {
                    fail!("{CONFIG}: step {name} is named twice");
                }
                steps.push(Step { name, lane, command, limit: None });
            }
            ("limit", _) => {
                let Some(limit) = parse_duration(value) else {
                    fail!("{CONFIG}: {line:?} needs a step and a time like 90s, 30m or 2h");
                };
                if limits.iter().any(|(step, _)| step == name) {
                    fail!("{CONFIG}: step {name} has two limits");
                }
                limits.push((name.to_string(), limit));
            }
            _ => fail!("{CONFIG}: no field in {line:?}"),
        }
    }
    for (name, limit) in limits {
        match steps.iter_mut().find(|step| step.name == name) {
            Some(step) => step.limit = Some(limit),
            None => fail!("{CONFIG}: limit for step {name}, which {CONFIG} does not name"),
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

/// Runs <command> in the checkout <top>, in a process group of its own, killed
/// once <limit> has passed.
pub fn run_command(top: &Path, command: &[String], limit: Option<Duration>) -> Result<Within> {
    let Some((program, args)) = command.split_first() else {
        fail!("{CONFIG}: a command names no program");
    };
    let path = if program.contains('/') { top.join(program) } else { PathBuf::from(program) };
    let mut start = Command::new(path);
    start.args(args).current_dir(top);
    lock::run_in_group_within(start, program, limit)
}

/// The folder of <repo>'s app checkout in <lane>.
pub fn app_checkout_top(places: &Places, repo: &str, lane: Lane) -> Result<PathBuf> {
    let Some(name) = places.app_checkout_of(repo, lane)? else {
        fail!("{repo} has no {} app checkout; builds app-checkout records one", lane.word());
    };
    let marker = places.root.join(repo).join(&name).join(".checkout");
    let text = std::fs::read_to_string(&marker).context(|| format!("reading {}", marker.display()))?;
    Ok(PathBuf::from(text.trim()))
}

#[cfg(test)]
#[path = "app_config_tests.rs"]
mod tests;
