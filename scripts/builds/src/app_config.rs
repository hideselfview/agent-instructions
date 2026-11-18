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
//! test android ./bae-android/test-class.sh {class}
//! shots mac scripts/cargo.sh run -p bae-shots -- run --platform mac --select {select}
//! shots-source bae-shots
//! ```
//!
//! A line starting with `#` is a comment.
//!
//! A `test <lane> <command>` line, at most one per lane, is the command that
//! runs test classes, `{class}` in a word standing for a class:
//! `builds app-build <repo> <ref> --test <lane> <Class>` runs it, so a class
//! needs no step of its own. builds runs the line once for all of a build's
//! classes that the line runs, and copies the word that holds `{class}` once
//! for each class: `--tests *{class}` becomes `--tests *A *B`, so a repo whose
//! program needs a flag before each class wraps the program in a script that
//! adds the flags. A `shots <lane> <command>` line is the same for a
//! selection of screenshots, `{select}` standing for the prefix of their
//! names: `--shots <lane> <prefix>` runs it, once for each prefix.
//!
//! A `shots-source <folder>` line names the folder, relative to the repo's top,
//! that holds the shots program's source. Its git tree at a commit is the
//! version of the checker that made a run's findings (shot_findings.rs); with
//! no such line, builds stores and reads no findings.
//!
//! Every other line whose keyword starts with `shots-` holds a screenshot
//! setting for builds-shots, the program `builds shots` runs, which reads
//! those lines itself (scripts/shots/src/settings.rs): this parser skips them.
//! A `results <lane> <path>...` line names folders of the lane's app checkout
//! (relative to it) that a run of tests or screenshots writes and agents read:
//! when the lane's checkout is on another host (app_remote.rs), a run of
//! `--test` or `--shots` copies them back to this Mac after it ends, passed
//! or failed. A lane may have several such lines.
//!
//! Each step and each test line has a place, the app checkouts it may run in
//! (`lock::Where`): any of the lane's, unless a line says otherwise. A `here
//! <step>...` line names steps that run only in an app checkout on this Mac,
//! never on a host (app_remote.rs): the signed macOS app for the person, whose
//! signing identity only this Mac has. An `opens-windows <lane>[:<program>]...`
//! line names test lines whose runs open windows (bae's UI tests), which run
//! only in an app checkout on a host: on this Mac the windows would cover the
//! person's screen. A lane's build takes a checkout where all its steps may
//! run.
//!
//! A `ci-only <step>...` line names steps that only CI runs, such as a whole
//! test suite or a lane's shots: `builds app-build` refuses such a step when a
//! caller names it, leaves it out when it runs every step of a lane, and
//! refuses an `on` or `needs` line that names it.
//!
//! Three kinds of line let `builds app-build` with no step named (derive.rs) choose the steps of a
//! branch, so no caller names one:
//!
//! ```text
//! on bae-macos/ macos-tests
//! on bae-bridge/ macos-tests android-bridge android
//! needs mac macos-tests
//! sources mac bae-macos BaeKit
//! sources mac:shots bae-macos/bae/baeShots
//! test mac:shots ./bae-macos/test.sh --bundle baeShots {class}
//! ```
//!
//! An `on <pattern> <step>...` line says that a changed path matching the
//! pattern needs those steps (a pattern ending in `/` or naming a path
//! matches the paths under or at it; one starting with `*` matches the paths
//! ending in the rest). A `needs <lane> <step>...` line names the steps a
//! test or shots run in the lane needs built first when the branch changes
//! nothing the lane's `on` lines match. A `sources <lane> <path>...` line
//! names the folders that declare the lane's tests, to find the lane of a
//! test class.
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

use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::lock::{self, Where, Within};
use crate::places::Places;
use boards::{git, git_text};

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
    /// How each lane runs test classes and one selection of shots: at most
    /// one test line per lane and program, and one shots line per lane.
    pub pickers: Vec<Picker>,
    /// The `shots-source` line: the folder of the shots program's source.
    pub shots_source: Option<String>,
    /// The folders each lane's runs of tests and screenshots write, from the
    /// `results` lines, in the order written.
    pub results: Vec<(Lane, String)>,
    /// The `on` lines: which changed paths need which steps.
    pub rules: Vec<Rule>,
    /// The `needs` lines: the steps a pick in the lane needs built first.
    pub needs: Vec<(Lane, Vec<String>)>,
    /// The `sources` lines: the folders that declare the lane's tests.
    pub sources: Vec<Source>,
}

/// An `on <pattern> <step>...` line.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Rule {
    pub pattern: String,
    pub steps: Vec<String>,
}

impl Rule {
    /// Does the changed path <path> (relative to the repo's top) match?
    pub fn matches(&self, path: &str) -> bool {
        match self.pattern.strip_prefix('*') {
            Some(ending) => path.ends_with(ending),
            None => path.starts_with(&self.pattern),
        }
    }
}

impl Config {
    /// The folders of <lane>'s checkout that its picks write.
    pub fn results_of(&self, lane: Lane) -> Vec<String> {
        self.results
            .iter()
            .filter(|(own, _)| *own == lane)
            .map(|(_, path)| path.clone())
            .collect()
    }

    /// The lane's default picker of <kind>: the line with no program.
    pub fn picker(&self, lane: Lane, kind: PickKind) -> Option<&Picker> {
        self.picker_in(lane, kind, None)
    }

    /// The lane's picker of <kind> for the named <program> (a `test
    /// <lane>:<program>` line), or the default one when <program> is None.
    pub fn picker_in(&self, lane: Lane, kind: PickKind, program: Option<&str>) -> Option<&Picker> {
        self.pickers
            .iter()
            .find(|picker| picker.is(lane, kind, program))
    }
}

/// A `sources <lane>[:<program>] <folder>...` line's folder: where the lane's
/// test classes are declared, and the program (a named `test` line) that runs
/// the classes declared there; None for the lane's default program.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Source {
    pub lane: Lane,
    pub program: Option<String>,
    pub folder: String,
}

/// <word> (`mac`, or `mac:shots` for a named program of the lane) as the lane
/// and the program.
fn lane_and_program(word: &str) -> Option<(Lane, Option<String>)> {
    match word.split_once(':') {
        None => Some((Lane::parse(word)?, None)),
        Some((lane, program)) => {
            let plain = !program.is_empty()
                && program
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            plain.then(|| Some((Lane::parse(lane)?, Some(program.to_string()))))?
        }
    }
}

/// The word `lane_and_program` reads: `mac` for <lane>'s default program,
/// `mac:shots` for the named <program>.
pub fn lane_and_program_word(lane: Lane, program: Option<&str>) -> String {
    match program {
        Some(program) => format!("{}:{program}", lane.word()),
        None => lane.word().to_string(),
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
    /// per-class and per-area steps in bae's `.app-build` had. A run of a
    /// test line has this limit however many classes it runs.
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
/// `Step`, that runs picks of the lane (the classes of a run, or one shots
/// prefix), with the kind's placeholder where each argument goes.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Picker {
    pub lane: Lane,
    pub kind: PickKind,
    /// The named program of a `test <lane>:<program>` line; None for the
    /// lane's default.
    pub program: Option<String>,
    pub command: Vec<String>,
    /// Where its runs may go: `Where::Hosts` for a test line an
    /// `opens-windows` line names, else `Where::Anywhere`.
    pub place: Where,
}

impl Picker {
    /// Is it the <lane>'s line of <kind> for <program> (None: the lane's
    /// default line)?
    pub fn is(&self, lane: Lane, kind: PickKind, program: Option<&str>) -> bool {
        self.lane == lane && self.kind == kind && self.program.as_deref() == program
    }

    /// The command for <args>: `command_for` copies each word that holds the
    /// placeholder once for each of <args>, in order, and puts that argument
    /// in place of the placeholder. Each argument stays one argument, which no
    /// shell reads.
    pub fn command_for(&self, args: &[&str]) -> Vec<String> {
        let placeholder = self.kind.placeholder();
        self.command
            .iter()
            .flat_map(|word| {
                if word.contains(placeholder) {
                    args.iter()
                        .map(|arg| word.replace(placeholder, arg))
                        .collect()
                } else {
                    vec![word.clone()]
                }
            })
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
    /// Where it may run: `Where::Here` for a step a `here` line names, the
    /// place of its test line for a test run, else `Where::Anywhere`.
    pub place: Where,
    /// True for a step a `ci-only` line names: only CI runs it, and
    /// `builds app-build` refuses it.
    pub ci_only: bool,
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
    let mut shots_source: Option<String> = None;
    let mut results: Vec<(Lane, String)> = Vec::new();
    let mut here: Vec<String> = Vec::new();
    let mut ci_only: Vec<String> = Vec::new();
    let mut windowed: Vec<(Lane, Option<String>)> = Vec::new();
    let mut rules: Vec<Rule> = Vec::new();
    let mut needs: Vec<(Lane, Vec<String>)> = Vec::new();
    let mut sources: Vec<Source> = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
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
                    place: Where::Anywhere,
                    ci_only: false,
                });
            }
            (keyword @ ("test" | "shots"), _) => {
                let kind = if keyword == "test" {
                    PickKind::Class
                } else {
                    PickKind::Shots
                };
                let placeholder = kind.placeholder();
                let Some((lane, program)) = lane_and_program(name) else {
                    fail!(
                        "{CONFIG}: {line:?}: a {keyword} line starts with its lane, mac or android (and for test, a program after a colon), not {name:?}"
                    );
                };
                if program.is_some() && kind != PickKind::Class {
                    fail!("{CONFIG}: {line:?}: only a test line names a program");
                }
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
                    .any(|picker| picker.is(lane, kind, program.as_deref()))
                {
                    fail!("{CONFIG}: two {keyword} lines for {name}");
                }
                pickers.push(Picker {
                    lane,
                    kind,
                    program,
                    command,
                    place: Where::Anywhere,
                });
            }
            ("shots-source", _) => {
                let folder = field.trim();
                if folder.is_empty() || folder.contains(char::is_whitespace) {
                    fail!("{CONFIG}: {line:?} needs one folder name");
                }
                if shots_source.replace(folder.to_string()).is_some() {
                    fail!("{CONFIG}: two shots-source lines");
                }
            }
            (keyword, _) if keyword.starts_with("shots-") => {}
            ("on", _) => {
                let mut named = words(field);
                if named.len() < 2 {
                    fail!("{CONFIG}: {line:?} needs a pattern and a step");
                }
                let pattern = named.remove(0);
                rules.push(Rule {
                    pattern,
                    steps: named,
                });
            }
            ("needs", _) => {
                let Some(lane) = Lane::parse(name) else {
                    fail!(
                        "{CONFIG}: {line:?}: a needs line starts with its lane, mac or android, not {name:?}"
                    );
                };
                let named = words(value);
                if named.is_empty() {
                    fail!("{CONFIG}: the needs line for {name} names no step");
                }
                needs.push((lane, named));
            }
            ("sources", _) => {
                let Some((lane, program)) = lane_and_program(name) else {
                    fail!(
                        "{CONFIG}: {line:?}: a sources line starts with its lane, mac or android, optionally with a program after a colon, not {name:?}"
                    );
                };
                let paths = words(value);
                if paths.is_empty() {
                    fail!("{CONFIG}: the sources line for {name} names no folder");
                }
                for path in paths {
                    if path.starts_with('/') || path.split('/').any(|part| part == "..") {
                        fail!(
                            "{CONFIG}: sources path {path:?} must be a folder inside the checkout"
                        );
                    }
                    sources.push(Source {
                        lane,
                        program: program.clone(),
                        folder: path,
                    });
                }
            }
            ("here", _) => {
                let named = words(field);
                if named.is_empty() {
                    fail!("{CONFIG}: a here line names no step");
                }
                here.extend(named);
            }
            ("ci-only", _) => {
                let named = words(field);
                if named.is_empty() {
                    fail!("{CONFIG}: a ci-only line names no step");
                }
                ci_only.extend(named);
            }
            ("opens-windows", _) => {
                let named = words(field);
                if named.is_empty() {
                    fail!("{CONFIG}: an opens-windows line names no test line");
                }
                for word in named {
                    let Some(test) = lane_and_program(&word) else {
                        fail!(
                            "{CONFIG}: {line:?}: an opens-windows line names test lines by lane, mac or android, with a program after a colon for a named one, not {word:?}"
                        );
                    };
                    windowed.push(test);
                }
            }
            ("results", _) => {
                let Some(lane) = Lane::parse(name) else {
                    fail!(
                        "{CONFIG}: {line:?}: a results line starts with its lane, mac or android, not {name:?}"
                    );
                };
                let paths = words(value);
                if paths.is_empty() {
                    fail!("{CONFIG}: the results line for {name} names no folder");
                }
                for path in paths {
                    if path.starts_with('/') || path.split('/').any(|part| part == "..") {
                        fail!(
                            "{CONFIG}: results path {path:?} must be a folder inside the checkout"
                        );
                    }
                    results.push((lane, path));
                }
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
    for name in &here {
        match steps.iter_mut().find(|step| step.name == *name) {
            Some(step) => step.place = Where::Here,
            None => fail!("{CONFIG}: here names step {name}, which {CONFIG} does not name"),
        }
    }
    for name in &ci_only {
        match steps.iter_mut().find(|step| step.name == *name) {
            Some(step) => step.ci_only = true,
            None => fail!("{CONFIG}: ci-only names step {name}, which {CONFIG} does not name"),
        }
    }
    for (lane, program) in windowed {
        let named = lane_and_program_word(lane, program.as_deref());
        match pickers
            .iter_mut()
            .find(|picker| picker.is(lane, PickKind::Class, program.as_deref()))
        {
            Some(picker) => picker.place = Where::Hosts,
            None => {
                fail!("{CONFIG}: opens-windows names {named}, which has no `test {named}` line")
            }
        }
    }
    for step in rules
        .iter()
        .flat_map(|rule| rule.steps.iter())
        .chain(needs.iter().flat_map(|(_, steps)| steps.iter()))
    {
        let Some(known) = steps.iter().find(|known| known.name == *step) else {
            fail!("{CONFIG}: a rule names step {step}, which {CONFIG} does not name");
        };
        if known.ci_only {
            fail!(
                "{CONFIG}: a rule names step {step}, which a ci-only line names, so no app build may run it"
            );
        }
    }
    for (lane, named) in &needs {
        for name in named {
            if steps
                .iter()
                .any(|step| step.name == *name && step.lane != *lane)
            {
                fail!(
                    "{CONFIG}: the needs line for {} names step {name}, which is in the other lane",
                    lane.word()
                );
            }
        }
    }
    for source in &sources {
        if let Some(program) = &source.program
            && !pickers
                .iter()
                .any(|picker| picker.is(source.lane, PickKind::Class, Some(program)))
        {
            let named = lane_and_program_word(source.lane, Some(program));
            fail!("{CONFIG}: sources for {named}, which has no `test {named}` line");
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
        shots_source,
        results,
        rules,
        needs,
        sources,
    })
}

/// Runs git in <top>, failing with what it said.
pub fn run_git(top: &Path, args: &[&str]) -> Result<String> {
    Ok(git_text(
        git(top).args(args),
        &format!("git {}", args.join(" ")),
    )?)
}

/// The full id of the commit <rev> names in the repository at <top>.
pub fn resolve(top: &Path, rev: &str) -> Result<String> {
    Ok(run_git(
        top,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )?
    .trim()
    .to_string())
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
    git_text(&mut command, &format!("git {}", args.join(" ")))?;
    Ok(())
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

/// The process that runs <command> in the checkout <top>.
fn start_of(top: &Path, command: &[String]) -> Result<Command> {
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
    Ok(start)
}

/// Runs <command> in the checkout <top>, in a process group of its own, killed
/// once <limit> has passed.
pub fn run_command(top: &Path, command: &[String], limit: Option<Duration>) -> Result<Within> {
    lock::run_in_group_within(start_of(top, command)?, &command[0], limit)
}

/// `run_command`, passing the command's output on as it arrives and keeping
/// what <keep> asks for.
pub fn run_command_keeping(
    top: &Path,
    command: &[String],
    limit: Option<Duration>,
    keep: lock::Keep,
) -> Result<Within> {
    lock::run_in_group_keeping(start_of(top, command)?, &command[0], limit, keep)
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
