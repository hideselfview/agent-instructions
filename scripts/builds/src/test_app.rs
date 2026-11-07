//! `builds test-app <repo> [--no-open] [<ref>]`: builds the person's test app
//! in the repo's app checkout, whose build stays warm, taking turns on its app
//! lock, and copies the app out, since the next build there replaces it.
//!
//! A repo's `.app-build` says what only it knows, one field per line, and
//! `builds app-build` (app_build.rs) reads its `step` lines:
//!
//! ```text
//! build ./bae-macos/run.sh --no-open
//! app bae-macos/bae/.build/runApp/Build/Products/Debug/bae.app
//! bundle fm.bae.desktop
//! step macos ./bae-macos/run.sh --no-open
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lock;
use crate::places::Places;
use crate::text::note;
use crate::tool::Tool;

pub const CONFIG: &str = ".app-build";
const USAGE: &str = "usage: builds test-app <repo> [--no-open] [<ref>]";
const QUIT_CHECKS: u32 = 60;
const QUIT_CHECK_EVERY: Duration = Duration::from_millis(500);

/// A repo's `.app-build`.
#[derive(Debug, PartialEq, Eq)]
pub struct Config {
    /// The build's program, relative to the checkout when it names a path,
    /// and its arguments.
    build: Vec<String>,
    /// The built app, relative to the checkout.
    app: String,
    /// The app's bundle id, to quit the one running.
    bundle: String,
    /// What `builds app-build` runs, in the order written.
    pub steps: Vec<Step>,
}

/// A `step <name> <program> [<args>...]` line: a program, relative to the
/// checkout when it names a path. No shell reads it: a step that needs one is
/// a script in the repo.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Step {
    pub name: String,
    pub command: Vec<String>,
}

pub fn parse_config(text: &str) -> Result<Config> {
    parse(text)
}

fn parse(text: &str) -> Result<Config> {
    let (mut build, mut app, mut bundle) = (None, None, None);
    let mut steps: Vec<Step> = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        match line.split_once(' ') {
            Some(("build", value)) => build = Some(value.split_whitespace().map(str::to_string).collect()),
            Some(("app", value)) => app = Some(value.trim().to_string()),
            Some(("bundle", value)) => bundle = Some(value.trim().to_string()),
            Some(("step", value)) => {
                let mut words = value.split_whitespace().map(str::to_string);
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
    match (build, app, bundle) {
        (Some(build), Some(app), Some(bundle)) => Ok(Config { build, app, bundle, steps }),
        _ => fail!("{CONFIG} needs build, app and bundle lines"),
    }
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
    parse(&std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?)
}

fn build(top: &Path, config: &Config) -> Result<lock::Ran> {
    run_command(top, &config.build)
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

/// Where the test app lives: outside the checkout, under the home folder.
fn copy_path(places: &Places, repo: &str, config: &Config) -> Result<PathBuf> {
    let Some(name) = Path::new(&config.app).file_name() else {
        fail!("{CONFIG}: app names no file");
    };
    Ok(places.home.join("Applications").join(format!("{repo}-test")).join(name))
}

fn with_new(app: &Path) -> PathBuf {
    let mut path = app.as_os_str().to_owned();
    path.push(".new");
    PathBuf::from(path)
}

/// Copies the built app beside the test app, as `<app>.new`.
fn copy_out(built: &Path, app: &Path) -> Result<()> {
    let folder = app.parent().expect("the test app has a folder");
    std::fs::create_dir_all(folder).context(|| format!("creating {}", folder.display()))?;
    let copy = with_new(app);
    if copy.exists() {
        std::fs::remove_dir_all(&copy).context(|| format!("removing {}", copy.display()))?;
    }
    let status = Command::new("ditto").arg(built).arg(&copy).status().context(|| "running ditto".to_string())?;
    if !status.success() {
        fail!("copying {} to {} failed", built.display(), copy.display());
    }
    Ok(())
}

/// Puts the copy in the test app's place.
fn replace(app: &Path) -> Result<()> {
    if app.exists() {
        std::fs::remove_dir_all(app).context(|| format!("removing {}", app.display()))?;
    }
    let copy = with_new(app);
    std::fs::rename(&copy, app).context(|| format!("moving {} to {}", copy.display(), app.display()))
}

fn running(bundle: &str) -> Result<bool> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(format!("application id \"{bundle}\" is running"))
        .output()
        .context(|| "running osascript".to_string())?;
    if !output.status.success() {
        fail!("asking whether {bundle} runs: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

/// Quits the running app, if one runs, and waits for it to go.
fn quit(bundle: &str, app: &Path) -> Result<()> {
    if !running(bundle)? {
        return Ok(());
    }
    note(format!("quitting the running {bundle}"));
    let status = Command::new("osascript")
        .arg("-e")
        .arg(format!("tell application id \"{bundle}\" to quit"))
        .status()
        .context(|| "running osascript".to_string())?;
    if !status.success() {
        fail!("asking {bundle} to quit failed; the new app is at {}", with_new(app).display());
    }
    for _ in 0..QUIT_CHECKS {
        if !running(bundle)? {
            return Ok(());
        }
        sleep(QUIT_CHECK_EVERY);
    }
    fail!("{bundle} is still running after being asked to quit; the new app is at {}", with_new(app).display())
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

pub fn cmd_test_app(tool: &Tool, args: &[String]) -> Result<()> {
    let Some((repo, rest)) = args.split_first() else {
        fail!("{USAGE}");
    };
    let mut open = true;
    let mut commit = "main";
    for arg in rest {
        match arg.as_str() {
            "--no-open" => open = false,
            other if other.starts_with('-') => fail!("{USAGE}"),
            other => commit = other,
        }
    }
    let places = &tool.places;
    let top = app_checkout_top(places, repo)?;

    note(format!("waiting for {repo}'s app lock, then building {commit}"));
    let mut held = lock::take(places, repo)?;
    // A takeover stops the build; it runs again from the checkout, behind the
    // build that took over.
    let config = loop {
        let config = check_out(&top, commit)?;
        match build(&top, &config)? {
            lock::Ran::Finished => break config,
            lock::Ran::TakenOver => {
                note(format!("the build was stopped for a takeover; building {commit} again after it"));
                held = held.requeue()?;
            }
        }
    };
    let app = copy_path(places, repo, &config)?;
    copy_out(&top.join(&config.app), &app)?;
    // The build and the copy are done: the next build can start.
    held.release();

    quit(&config.bundle, &app)?;
    replace(&app)?;
    note(format!("built {}", app.display()));
    if open {
        let status = Command::new("open").arg(&app).status().context(|| "running open".to_string())?;
        if !status.success() {
            fail!("opening {} failed", app.display());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "test_app_tests.rs"]
mod tests;
