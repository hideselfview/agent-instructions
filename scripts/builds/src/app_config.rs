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
//! test mac ./bae-macos/test.sh --run-only {class}
//! test android ./bae-android/gradle.sh :app:testFullDebugUnitTest --tests *{class}
//! shots mac scripts/cargo.sh run -p bae-shots -- run --platform mac --select {select}
//! ```
//!
//! A `test <lane> <command>` line, at most one per lane, is the command that
//! runs one test class, `{class}` in a word standing for the class:
//! `builds app-build <repo> <ref> --test <lane> <Class>` runs it, so a class
//! needs no step of its own. A `shots <lane> <command>` line is the same for
//! a selection of screenshots, `{select}` standing for the prefix of their
//! names: `--shots <lane> <prefix>` runs it.
//!
//! A step belongs to a lane (`mac` or `android`, lane.rs), named first: the
//! lane's app checkout and lock are where it runs.
//!
//! A step runs for at most `DEFAULT_STEP_LIMIT`, or the time a `limit <step>
//! <duration>` line gives it (`90s`, `30m`, `2h`).

use std::os::unix::process::CommandExt;
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
    /// How each lane runs one test class and one selection of shots; at most
    /// one of each per lane.
    pub pickers: Vec<Picker>,
}

impl Config {
    pub fn picker(&self, lane: Lane, kind: PickKind) -> Option<&Picker> {
        self.pickers
            .iter()
            .find(|picker| picker.lane == lane && picker.kind == kind)
    }
}

/// What a command-line flag picks to run through a `.app-build` line of the
/// lane, in place of a step of its own.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PickKind {
    /// `--test <lane> <Class>`, run by the `test` line.
    Class,
    /// `--shots <lane> <prefix>`, run by the `shots` line.
    Shots,
}

impl PickKind {
    pub const ALL: [PickKind; 2] = [PickKind::Class, PickKind::Shots];

    /// The `.app-build` line that runs it.
    pub fn line(self) -> &'static str {
        match self {
            PickKind::Class => "test",
            PickKind::Shots => "shots",
        }
    }

    /// The command-line flag that picks it.
    pub fn flag(self) -> &'static str {
        match self {
            PickKind::Class => "--test",
            PickKind::Shots => "--shots",
        }
    }

    /// What the flag's argument is called in messages.
    pub fn what(self) -> &'static str {
        match self {
            PickKind::Class => "class",
            PickKind::Shots => "prefix",
        }
    }

    /// The text in the line's command that the argument takes the place of.
    pub fn placeholder(self) -> &'static str {
        match self {
            PickKind::Class => "{class}",
            PickKind::Shots => "{select}",
        }
    }

    /// How long one run may take, as the limit of a step: the time the
    /// per-class and per-area steps in bae's `.app-build` had.
    pub fn limit(self) -> Duration {
        match self {
            PickKind::Class => Duration::from_secs(10 * 60),
            PickKind::Shots => Duration::from_secs(20 * 60),
        }
    }

    /// The start of the name a run goes by in messages and the dashboard's
    /// step lines: `test Foo`, `shots mac.welcome.`.
    pub fn step_prefix(self) -> &'static str {
        match self {
            PickKind::Class => "test ",
            PickKind::Shots => "shots ",
        }
    }
}

/// A `test` or `shots` `<lane> <program> [<args>...]` line: the command, as for
/// `Step`, that runs one pick of the lane, with the kind's placeholder where
/// the argument goes.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Picker {
    pub lane: Lane,
    pub kind: PickKind,
    pub command: Vec<String>,
}

impl Picker {
    /// The command with <arg> in each word's placeholder; the argument stays
    /// one argument, never read by a shell.
    pub fn command_for(&self, arg: &str) -> Vec<String> {
        self.command
            .iter()
            .map(|word| word.replace(self.kind.placeholder(), arg))
            .collect()
    }
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
    let mut pickers: Vec<Picker> = Vec::new();
    let mut limits: Vec<(String, Duration)> = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (platform, field) = line.split_once(' ').unwrap_or((line, ""));
        let (name, value) = field.trim().split_once(' ').unwrap_or((field.trim(), ""));
        let value = value.trim();
        match (platform, name) {
            ("mac", "build") => mac_build = Some(words(value)),
            ("mac", "app") => mac_app = Some(value.to_string()),
            ("mac", "bundle") => bundle = Some(value.to_string()),
            ("android", "build") => {
                android = Some(AndroidApp {
                    build: words(value),
                })
            }
            ("step", _) => {
                let mut words = field.split_whitespace().map(str::to_string);
                let (Some(lane), Some(name)) = (words.next(), words.next()) else {
                    fail!("{CONFIG}: a step needs a lane, a name and a command");
                };
                let command = words.collect::<Vec<_>>();
                let Some(lane) = Lane::parse(&lane) else {
                    fail!(
                        "{CONFIG}: {line:?}: a step starts with its lane, mac or android, not {lane:?}"
                    );
                };
                if command.is_empty() {
                    fail!("{CONFIG}: step {name} names no command");
                }
                if steps.iter().any(|step| step.name == name) {
                    fail!("{CONFIG}: step {name} is named twice");
                }
                steps.push(Step {
                    name,
                    lane,
                    command,
                    limit: None,
                });
            }
            (keyword @ ("test" | "shots"), _) => {
                let kind = if keyword == "test" {
                    PickKind::Class
                } else {
                    PickKind::Shots
                };
                let placeholder = kind.placeholder();
                let Some(lane) = Lane::parse(name) else {
                    fail!(
                        "{CONFIG}: {line:?}: a {keyword} line starts with its lane, mac or android, not {name:?}"
                    );
                };
                let command = words(value);
                if command.is_empty() {
                    fail!("{CONFIG}: the {keyword} line for {name} names no command");
                }
                if !command.iter().any(|word| word.contains(placeholder)) {
                    fail!(
                        "{CONFIG}: the {keyword} line for {name} has no {placeholder} for the {} to go in",
                        kind.what()
                    );
                }
                if pickers
                    .iter()
                    .any(|picker| picker.lane == lane && picker.kind == kind)
                {
                    fail!("{CONFIG}: two {keyword} lines for {name}");
                }
                pickers.push(Picker {
                    lane,
                    kind,
                    command,
                });
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
    Ok(Config {
        mac,
        android,
        steps,
        pickers,
    })
}

/// Runs git in <top>, failing with what it said.
pub fn run_git(top: &Path, args: &[&str]) -> Result<String> {
    git_text(git(top).args(args), &format!("git {}", args.join(" ")))
}

/// Detaches the app checkout <top> at <commit> (`force`: over local changes),
/// so that the tree is the old commit's or the new one's however the build
/// that asks is stopped. `git checkout` writes the files first and moves HEAD
/// last, so a checkout killed in between leaves the new commit's files under
/// the old HEAD, a tree every later build refuses as dirty. So git runs in a
/// process group of its own, with the stopping signals blocked (a Ctrl-C or a
/// SIGTERM to the build's group, a takeover's SIGUSR2) and SIGPIPE ignored (the
/// build may be gone when git writes). It holds the checkout's lock too: the
/// lock's descriptor is inherited, so the next build waits for git, not for the
/// build that started it. Only SIGKILL stops it, and `builds app-checkout reset`
/// puts a checkout that was killed that way right.
pub fn switch_to(top: &Path, commit: &str, force: bool) -> Result<()> {
    let mut args = vec!["checkout", "-q", "--detach"];
    if force {
        args.push("--force");
    }
    args.push(commit);
    let mut command = git(top);
    command.args(&args).process_group(0);
    unsafe {
        command.pre_exec(|| {
            let mut stopping = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut stopping);
            for signal in [
                libc::SIGTERM,
                libc::SIGINT,
                libc::SIGHUP,
                libc::SIGQUIT,
                libc::SIGUSR1,
                libc::SIGUSR2,
            ] {
                libc::sigaddset(&mut stopping, signal);
            }
            libc::sigprocmask(libc::SIG_BLOCK, &stopping, std::ptr::null_mut());
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            Ok(())
        });
    }
    git_text(&mut command, &format!("git {}", args.join(" "))).map(|_| ())
}

/// The ref checked out in the app checkout <top>, which must be clean, and its
/// `.app-build`.
pub fn check_out(top: &Path, commit: &str) -> Result<Config> {
    let changes = run_git(top, &["status", "--porcelain"])?;
    if !changes.trim().is_empty() {
        fail!(
            "the app checkout has changes, so nothing was built (if a switch was cut short, `builds app-checkout reset <repo> <checkout>` clears it when every change is a commit's):\n{changes}"
        );
    }
    run_git(top, &["fetch", "-q", "origin"])?;
    switch_to(top, commit, false)?;
    let file = top.join(CONFIG);
    parse_config(&std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?)
}

/// Runs <command> in the checkout <top>, in a process group of its own, killed
/// once <limit> has passed.
pub fn run_command(top: &Path, command: &[String], limit: Option<Duration>) -> Result<Within> {
    let Some((program, args)) = command.split_first() else {
        fail!("{CONFIG}: a command names no program");
    };
    let path = if program.contains('/') {
        top.join(program)
    } else {
        PathBuf::from(program)
    };
    let mut start = Command::new(path);
    start.args(args).current_dir(top);
    lock::run_in_group_within(start, program, limit)
}

/// The folder of <repo>'s app checkout <name>.
pub fn app_checkout_top(places: &Places, repo: &str, name: &str) -> Result<PathBuf> {
    let marker = places.root.join(repo).join(name).join(".checkout");
    let text =
        std::fs::read_to_string(&marker).context(|| format!("reading {}", marker.display()))?;
    Ok(PathBuf::from(text.trim()))
}

#[cfg(test)]
#[path = "app_config_tests.rs"]
mod tests;
