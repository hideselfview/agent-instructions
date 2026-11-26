//! `builds app-build <repo> <ref> [<step>...]`: builds <ref> in the repo's app
//! checkouts with the steps its `.app-build` names. A step belongs to a lane
//! (lane.rs), and each lane has an app checkout, lock and queue of its own, so
//! the lanes the steps are in build at the same time and wait for no one but
//! their own queue.
//!
//! The command starts one `builds app-build <repo> --lane <lane> <ref>
//! [<step>...]` process per lane, each with its queue ticket (so a takeover or
//! `top` names that process) and no stdin, and prefixes every line of their
//! output with `[<lane>] `, which becomes `[<lane> <checkout>] ` once the
//! lane's process holds a lock and so knows the checkout it builds in (it says
//! so in a marker line the parent reads and does not print). Each lane runs the steps
//! named that are in it, in the order named. It exits 1 if either lane fails, saying which.
//!
//! A lane's process takes the first free lock among the lane's app checkouts
//! (queue.rs: first come, first served across the set), checks <ref> out in
//! that checkout (which must be clean) and runs the lane's steps each through
//! `run_in_group`, stopping at the first one that fails. A step has a time
//! limit (app_config.rs): past it the step's process group is killed and the
//! build fails, which gives up the lock.
//! With no step named, `builds app-build <repo> <ref> [--test <Class>]...
//! [--shots <prefix>]... [--fresh]` chooses the steps, the lanes and the
//! checkouts itself from the branch's diff (derive.rs); the rest of this
//! comment is the form that names steps.
//! `builds app-build <repo> --list` prints the steps and the test and shots
//! lines.
//!
//! `--test <lane> <Class>` and `--shots <lane> <prefix>` (any number of each,
//! in any order, after the steps) add, to the end of that lane's steps, the
//! lane's `test` or `shots` line (app_config.rs) with the classes or the
//! prefix in it, each run with the kind's limit (a shots run of the checker
//! also has the time of one scene for each scene it takes, which a count made
//! in the checkout before the run gives, app_config.rs `Scenes`): one run of each test line
//! for all of its classes, and one run of the shots line per prefix. With
//! such flags and no step named in the `--lane` form, only those runs happen.
//! An app build runs only what a change touches, so every form refuses a
//! `--shots` prefix that selects every shot of its lane (`mac.`) and a step
//! that a `ci-only` line names (app_config.rs); CI runs the full suites.
//! `--all-shots <lane>` is how a caller asks for a lane's full set of shots,
//! to check a change to the shots program: the run of the lane's `shots` line
//! with the prefix every shot of the lane starts with, cached like any other
//! shots run under that prefix.
//!
//! A run of picks with no step named tests what the checkout holds, so it
//! refuses unless the checkout's last build (`Built`, kept beside the lock's
//! checkout folder, written when a run's steps all finish and removed when a
//! run with steps starts) is of the commit it names and the checkout is still
//! on that commit; it names what was built instead.
//!
//! A lane's checkout can be on a host (app_remote.rs): the lock and the queue
//! stay here, and the host's checkout is switched to the commit and runs the
//! steps through `builds remote step`, with the same time limits and
//! takeovers. A lane's build takes the free lock of the less loaded host's
//! checkout first (lock.rs). After each `--test` or `--shots` run on a host,
//! the `results` lines of `.app-build` name the folders copied back here.
//!
//! A lane reads the environment that draws its shots (the macOS and Xcode
//! builds on a Mac, the OS and architecture for android) from the host of the
//! checkout it holds (app_platform.rs). That environment keys the runs it
//! caches and the pictures it takes, and a shots run compares only with a run
//! made in the same environment.
//!
//! A shots run of a branch compares with the cached run of the branch's merge
//! base with origin/main (shots_cache.rs). When the cache lacks the run of a
//! prefix that starts a shot declared at the merge base, the lane's process
//! builds the merge base first, in the same checkout and the same turn of the
//! queue (`LaneBuild::ensure_base`): it holds a lock on the lane's runs of that
//! merge base, runs the lane's `needs` steps there and the missing prefixes
//! with `--base`, and then builds the branch. A build never asks a branch to
//! rebase for a comparison.
//!
//! A lane with two or more shots prefixes starts the app's capture once: a
//! capture step runs the shots line with every prefix and `--capture-only`
//! (`capture_step`), and each prefix's run then takes its shots from the
//! picture folder with `--no-capture`.
//!
//! A takeover stops the step running; the lane's build then runs again from
//! the checkout, behind the build that took over.
//!
//! A stop signal (SIGTERM, SIGINT or SIGHUP) ends the build. The command
//! passes it on to each lane's process, and each lane, which answers it from
//! before it joins the queue (lock.rs `end_on_stop_signals`), passes it on to
//! the step that runs: a step on a host stops there. A lane that waits, for
//! its turn in the queue or for a host's display, ends the wait, and a lane
//! between two steps starts no other. The lane then puts the checkout back
//! on its commit if the steps changed it, gives up its locks and records the
//! run as stopped, with the step the signal ended as stopped. The command
//! says it stopped once the lanes have ended.
//!
//! An install never stops a lane: the lane builds to its end on the programs
//! the build started with (programs.rs), and when the install moved the
//! stores meanwhile, its run reaches the store through the installed builds
//! (handed_runs.rs).
//!
//! When a step of the mac lane fails after xcodebuild printed that it cannot
//! start (xcode.rs), or after XCTest printed that it timed out turning on
//! automation mode (ui_test_settings.rs), the host caused the failure, not
//! the code. The lane then records the host as refusing mac app builds
//! (host.rs), and `builds health` lists the host with what fixes it: the fix
//! xcodebuild named, or the commands that turn on the settings that UI tests need.
//! The lane runs again from its place in the queue, in a checkout on another
//! host, because the lane's lock skips checkouts on hosts recorded this way.

use std::io::{BufRead, BufReader, Read, Write};
use std::num::NonZeroU8;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::app_build_step_failure::StepRead;
use crate::app_build_step_shots::{BaseRun, Pictures, ReusedRead, ShotsUse};
use crate::app_config::{
    CONFIG, CheckedOut, Config, PickKind, Picker, Scenes, Step, check_out, config_in, count_scenes,
    parse_config, parse_scene_count, resolve, resolve_checkout, run_command_keeping, show_duration,
};
use crate::app_landing::covers_prefix;
use crate::app_lock_guard::CheckoutLock;
use crate::app_remote::{RemoteApp, fetched_local_source, local_source};
use crate::app_site::Site;
use crate::app_timing::{
    BuildRecords, BuiltCommit, RunRecord, StepKind, StepRecord, StepResult, Timer,
};
use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::host::{Refused, Ssh};
use crate::install_gate;
use crate::lane::Lane;
use crate::lock::{self, Found, Keep, Ran, Within};
use crate::places::Places;
use crate::programs::Pinned;
use crate::run_end::RunEnd;
use crate::shot_findings;
use crate::shots_cache;
use crate::shots_cache::MissCause;
use crate::shots_declared::ShotDeclarations;
use crate::text::note;
use crate::tool::Tool;
use crate::window_lock::Access;

const USAGE: &str = "usage: builds app-build <repo> <ref> [--test <Class>]... [--shots <prefix>]... [--all-shots <app>]... [--fresh] [--project P] | builds app-build <repo> <ref> <step>... [--test <app> <Class>]... [--shots <app> <prefix>]... [--all-shots <app>]... [--fresh] [--project P] | builds app-build <repo> --list | builds app-build <repo> --lane <app> <ref> [<step>...] [--test <app> <Class>]... [--shots <app> <prefix>]... [--all-shots <app>]...";

/// Set by the parent in each lane's process: print the marker line below once
/// the checkout is known.
const PREFIXED: &str = "BUILDS_APP_BUILD_PREFIXED";
/// Set by the parent to the lane's list of shots comparisons it reused.
const REUSED_SHOTS: &str = "BUILDS_APP_BUILD_REUSED_SHOTS";
/// A line of a lane's output that names the checkout it builds in; the parent
/// changes the prefix and prints nothing.
const CHECKOUT_MARKER: &str = "\u{1}builds-app-checkout ";

/// The prefix for <lane>'s lines once <line> is the marker naming a checkout;
/// None for any other line.
fn prefix_from_marker(lane: &str, line: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(line).ok()?;
    let name = text.strip_prefix(CHECKOUT_MARKER)?.trim_end();
    Some(format!("[{lane} {name}] "))
}

/// How long the parent waits, after a lane's process has ended, for the end of
/// its output: a step may leave a process behind (a build server) that holds
/// the pipe open.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// Returns the step a caller named <name>, and fails when <config> has no
/// such step or a `ci-only` line names it.
fn step_named<'a>(config: &'a Config, name: &str) -> Result<&'a Step> {
    let Some(step) = config.steps.iter().find(|step| step.name == name) else {
        let known: Vec<&str> = config.steps.iter().map(|step| step.name.as_str()).collect();
        fail!("{CONFIG} has no step {name} (it has: {})", known.join(", "));
    };
    if step.ci_only {
        fail!(
            "step {name} is CI-only in {CONFIG}, and builds refuses it: {ONLY_WHAT_A_CHANGE_TOUCHES}. Name the change's test classes with --test and its screens' shots with --shots, or run a lane's full set of shots with {ALL_SHOTS} <app>"
        );
    }
    Ok(step)
}

/// The steps of <lane> to run: those named that are in it, in the order
/// named, or all of the lane's that are not CI-only, in the order written,
/// when none is named.
fn chosen(config: &Config, lane: Lane, names: &[String]) -> Result<Vec<Step>> {
    if names.is_empty() {
        return Ok(config
            .steps
            .iter()
            .filter(|step| step.lane == lane && !step.ci_only)
            .cloned()
            .collect());
    }
    names
        .iter()
        .map(|name| {
            let step = step_named(config, name)?;
            if step.lane != lane {
                fail!(
                    "step {name} is in the {} lane, not the {} lane",
                    step.lane.word(),
                    lane.word()
                );
            }
            Ok(step.clone())
        })
        .collect()
}

/// A `--test <lane> <Class>` or `--shots <lane> <prefix>` flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Class(Lane, String),
    Shots(Lane, shots::Prefix),
}

impl Pick {
    pub fn from_flag(lane: Lane, kind: PickKind, arg: &str) -> Result<Self> {
        match kind {
            PickKind::Class => Ok(Self::Class(lane, arg.to_string())),
            PickKind::Shots => {
                refuse_whole_lane(lane, arg)?;
                let prefix =
                    shots::Prefix::parse(arg).map_err(|error| Fail::Message(error.to_string()))?;
                Ok(Self::Shots(lane, prefix))
            }
        }
    }

    pub fn lane(&self) -> Lane {
        match self {
            Self::Class(lane, _) | Self::Shots(lane, _) => *lane,
        }
    }

    pub fn kind(&self) -> PickKind {
        match self {
            Self::Class(..) => PickKind::Class,
            Self::Shots(..) => PickKind::Shots,
        }
    }

    pub fn argument(&self) -> &str {
        match self {
            Self::Class(_, class) => class,
            Self::Shots(_, prefix) => prefix.as_str(),
        }
    }

    pub fn is_all_shots(&self) -> bool {
        matches!(self, Self::Shots(lane, prefix) if prefix.as_str() == lane.all_shots_prefix())
    }

    pub fn prefixes(picks: &[Self]) -> impl Iterator<Item = &shots::Prefix> {
        picks.iter().filter_map(|pick| match pick {
            Self::Class(..) => None,
            Self::Shots(_, prefix) => Some(prefix),
        })
    }
}

/// The rule that app-build names when it refuses a `--test` argument that is
/// no type name, a `--shots` prefix that selects every shot of its lane, or a
/// step that a `ci-only` line names.
pub const ONLY_WHAT_A_CHANGE_TOUCHES: &str = "an app build runs only the test classes and shots of the screens a change touches, and CI runs the full suites";

/// Refuses a shots <prefix> in <lane> that selects every shot of the lane:
/// one that `<lane>.` starts with (`mac.`, `mac`), since every shot name of
/// the lane starts with `<lane>.`. `--all-shots <lane>` asks for that run.
fn refuse_whole_lane(lane: Lane, prefix: &str) -> Result<()> {
    let word = lane.word();
    if lane.all_shots_prefix().starts_with(prefix) {
        fail!(
            "the shots prefix {prefix} selects every shot of the {word} lane, and builds refuses it in --shots: {ONLY_WHAT_A_CHANGE_TOUCHES}. Give a prefix that names the screens the change touches, such as {word}.welcome. for the welcome screens. To check a change to the shots program, {ALL_SHOTS} {word} runs every shot of the lane"
        );
    }
    Ok(())
}

/// The flag that asks for a lane's full set of shots: `--all-shots mac`.
pub const ALL_SHOTS: &str = "--all-shots";

/// The pick of <lane>'s full set of shots: the run of the lane's `shots` line
/// with the prefix every shot of the lane starts with.
pub fn all_shots(lane: Lane) -> Pick {
    Pick::Shots(lane, shots::Prefix::all_of(&lane.app()))
}

/// The pick of the `--all-shots <lane>` flag whose words after the flag are
/// <after>, and the words after the lane; fails naming <usage> when no lane
/// follows.
pub fn all_shots_flag<'a>(
    after: &'a [String],
    usage: &str,
    lanes: &[Lane],
) -> Result<(Pick, &'a [String])> {
    let [lane, rest @ ..] = after else {
        fail!("{ALL_SHOTS} needs an app\n{usage}");
    };
    Ok((all_shots(Lane::parse_in_or_fail(lanes, lane)?), rest))
}

/// The command-line words that ask for <pick> in a lane's process, or in a
/// request that names steps: `--all-shots <lane>` for a lane's full set of
/// shots, else the kind's flag, the lane and the argument.
pub fn pick_words(pick: &Pick) -> Vec<String> {
    let lane = pick.lane();
    let words = if pick.is_all_shots() {
        vec![ALL_SHOTS, lane.word()]
    } else {
        vec![pick.kind().flag(), lane.word(), pick.argument()]
    };
    words.into_iter().map(str::to_string).collect()
}

/// The kind of step <name> is, when it is the run of a pick.
pub fn pick_kind_of(name: &str) -> Option<PickKind> {
    PickKind::ALL
        .into_iter()
        .find(|kind| name.starts_with(kind.step_prefix()))
}

/// <args> (the steps, then `--test` and `--shots` flags, each with a lane and
/// an argument, and `--all-shots` flags, each with a lane) as the step names
/// and the flags.
pub fn split_picks(args: &[String], lanes: &[Lane]) -> Result<(Vec<String>, Vec<Pick>)> {
    let is_flag =
        |arg: &String| arg == ALL_SHOTS || PickKind::ALL.iter().any(|kind| arg == kind.flag());
    let at = args.iter().position(is_flag).unwrap_or(args.len());
    let (names, mut flags) = (args[..at].to_vec(), &args[at..]);
    let mut picks = Vec::new();
    while let Some((flag, rest)) = flags.split_first() {
        if flag == ALL_SHOTS {
            let (pick, after) = all_shots_flag(rest, USAGE, lanes)?;
            picks.push(pick);
            flags = after;
            continue;
        }
        let Some(kind) = PickKind::ALL.into_iter().find(|kind| flag == kind.flag()) else {
            fail!("{flag:?} where --test, --shots or {ALL_SHOTS} goes\n{USAGE}");
        };
        let [lane, arg, after @ ..] = rest else {
            fail!(
                "{} needs a lane and a {}\n{USAGE}",
                kind.flag(),
                kind.what()
            );
        };
        if arg.is_empty() || arg.starts_with('-') || arg.contains(char::is_whitespace) {
            fail!(
                "{} {lane} takes one {} name, not {arg:?}",
                kind.flag(),
                kind.what()
            );
        }
        let lane = Lane::parse_in_or_fail(lanes, lane)?;
        picks.push(Pick::from_flag(lane, kind, arg)?);
        flags = after;
    }
    Ok((names, picks))
}

/// The refusal for a pick of a kind whose line <config> lacks in <lane>.
fn missing_line(lane: Lane, kind: PickKind, arg: &str) -> Fail {
    Fail::Message(format!(
        "{CONFIG} has no `{} {} ...` line, so it cannot run the {} {arg}",
        kind.line(),
        lane.word(),
        kind.what()
    ))
}

/// Fails when the config has no line that runs a requested pick.
pub fn require_pickers(config: &Config, picks: &[Pick]) -> Result<()> {
    for pick in picks {
        let (lane, kind) = (pick.lane(), pick.kind());
        if config.picker(lane, kind).is_none() {
            return Err(missing_line(lane, kind, pick.argument()));
        }
    }
    Ok(())
}

/// One run of a `test` or `shots` line: the line, and its arguments (every
/// class of the lane's picks that the line runs, or one shots prefix).
struct PickRun<'a> {
    picker: &'a Picker,
    args: Vec<&'a str>,
}

/// What <lane> runs: its steps (<names>, or every one when none is named and
/// no pick is), then the runs of its lines for <picks>. A test class runs
/// through the program <program_of> names for it, the lane's default when
/// None, and each program runs all of its classes in one invocation, placed
/// in the order of its first class: each start of Gradle or xcodebuild takes
/// seconds before any test runs. Each shots prefix runs on its own, since
/// builds caches the findings of each prefix's run; a lane with two or more
/// prefixes first takes all their shots in one capture step (`capture_step`),
/// and each prefix's run then reads them from the picture folder with
/// `--no-capture`. <windowed> gives each prefix's display access, when it
/// captures on the host's display (`shots_declared::windowed_prefixes`).
fn lane_steps(
    config: &Config,
    lane: Lane,
    names: &[String],
    picks: &[Pick],
    windowed: &[(String, Access)],
    program_of: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<Vec<Step>> {
    lane_steps_with(config, lane, names, picks, windowed, false, program_of)
}

/// `lane_steps` for a lane whose picture folder caches every shot of its
/// picks when <folder_caches>: then no step captures, since no app is built.
fn lane_steps_with(
    config: &Config,
    lane: Lane,
    names: &[String],
    picks: &[Pick],
    windowed: &[(String, Access)],
    folder_caches: bool,
    program_of: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<Vec<Step>> {
    let mut steps = if names.is_empty() && !picks.is_empty() {
        Vec::new()
    } else {
        chosen(config, lane, names)?
    };
    let mut runs: Vec<PickRun> = Vec::new();
    for pick in picks.iter().filter(|pick| pick.lane() == lane) {
        let kind = pick.kind();
        let arg = pick.argument();
        let program = match kind {
            PickKind::Class => program_of(arg)?,
            PickKind::Shots => None,
        };
        let Some(picker) = config.picker_in(lane, kind, program.as_deref()) else {
            return Err(missing_line(lane, kind, arg));
        };
        let same = runs
            .iter_mut()
            .find(|run| picker.kind == PickKind::Class && std::ptr::eq(run.picker, picker));
        match same {
            Some(run) => run.args.push(arg),
            None => runs.push(PickRun {
                picker,
                args: vec![arg],
            }),
        }
    }
    // A lane with several shots prefixes takes all their shots in one run of
    // the shots line, which each prefix's run then reads from the picture
    // folder: each start of the app's capture (Gradle, xcodebuild) takes
    // seconds before any shot.
    let prefixes: Vec<&str> = runs
        .iter()
        .filter(|run| run.picker.kind == PickKind::Shots)
        .flat_map(|run| run.args.iter().copied())
        .collect();
    let captures_once = prefixes.len() >= 2;
    let mut captured = false;
    for run in runs {
        let kind = run.picker.kind;
        if captures_once && kind == PickKind::Shots && !captured {
            captured = true;
            if !folder_caches {
                steps.push(capture_step(lane, run.picker, &prefixes, windowed)?);
            }
        }
        let mut command = run.picker.command_for(&run.args);
        let scenes = match kind {
            PickKind::Shots => shots_scenes(&command),
            PickKind::Class => None,
        };
        let reads_pictures = (captures_once || folder_caches) && kind == PickKind::Shots;
        if reads_pictures {
            command.push(NO_CAPTURE.to_string());
        }
        steps.push(Step {
            name: format!("{}{}", kind.step_prefix(), run.args.join(" ")),
            lane,
            limit: Some(kind.limit()),
            scenes,
            command,
            place: lock::Where::Anywhere,
            window: match kind {
                PickKind::Shots if !reads_pictures => window_access(&run.args, windowed),
                PickKind::Shots => None,
                PickKind::Class => run.picker.opens_windows.then_some(Access::Exclusive),
            },
            ci_only: false,
        });
    }
    Ok(steps)
}

/// The scenes a shots run of <command> takes, which its limit grows with
/// (`PickKind::scene_time`); none for a line that is not the checker.
fn shots_scenes(command: &[String]) -> Option<Scenes> {
    PickKind::Shots
        .scene_time()
        .zip(crate::app_config::counting(command))
        .map(|(each, count)| Scenes { count, each })
}

/// The strongest display access that a step's selected prefixes need.
fn window_access(args: &[&str], prefixes: &[(String, Access)]) -> Option<Access> {
    prefixes
        .iter()
        .filter(|(prefix, _)| args.contains(&prefix.as_str()))
        .map(|(_, access)| *access)
        .max()
}

/// The start of the name of a lane's capture step (`capture_step`).
const CAPTURE_STEP: &str = "capture shots ";

/// The flag of `builds shots run` that makes a run fail instead of capturing
/// when the picture folder lacks a shot it takes.
const NO_CAPTURE: &str = "--no-capture";

/// The flag of `builds shots run` that takes the selected shots into the
/// picture folder and ends there.
const CAPTURE_ONLY: &str = "--capture-only";

/// Whether <step> reads or writes the picture folder: a shots run or a lane's
/// capture step.
fn takes_pictures(step: &Step) -> bool {
    pick_kind_of(&step.name) == Some(PickKind::Shots) || is_capture(&step.name)
}

/// Whether the step named <name> is a lane's capture step.
pub fn is_capture(name: &str) -> bool {
    name.starts_with(CAPTURE_STEP)
}

/// The step that takes the shots of all of <prefixes> into the picture folder
/// in one run of the lane's `shots` line <picker>, which the runs of the
/// prefixes then read (`lane_steps`). It opens windows when a prefix
/// selects a scene shown in its window (<windowed>), and its limit grows with
/// the scenes of every prefix.
fn capture_step(
    lane: Lane,
    picker: &Picker,
    prefixes: &[&str],
    windowed: &[(String, Access)],
) -> Result<Step> {
    let mut command = picker.command_repeating(prefixes)?;
    let scenes = shots_scenes(&command);
    command.push(CAPTURE_ONLY.to_string());
    Ok(Step {
        name: format!("{CAPTURE_STEP}{}", prefixes.join(" ")),
        lane,
        limit: Some(PickKind::Shots.limit()),
        scenes,
        command,
        place: lock::Where::Anywhere,
        window: window_access(prefixes, windowed),
        ci_only: false,
    })
}

/// Where a lane's checkout must be to run all of <steps>: on this Mac when
/// any of them runs only there, else anywhere.
pub fn place_of(steps: &[Step]) -> lock::Where {
    if steps.iter().any(|step| step.place == lock::Where::Here) {
        lock::Where::Here
    } else {
        lock::Where::Anywhere
    }
}

/// The names in <names> that are steps of <lane> in <config>, in order.
fn names_in(config: &Config, lane: Lane, names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| {
            config
                .steps
                .iter()
                .any(|step| step.name == **name && step.lane == lane)
        })
        .cloned()
        .collect()
}

/// What an app checkout last built: the commit (full id) and the steps that
/// finished on it, in the order run. Kept in `Places::app_built_file`.
#[derive(Debug, PartialEq, Eq)]
struct Built {
    commit: String,
    steps: Vec<String>,
}

impl Built {
    fn read(file: &Path) -> Result<Option<Built>> {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => fail!("reading {}: {error}", file.display()),
        };
        let mut lines = text.lines();
        let Some(commit) = lines.next().filter(|line| !line.is_empty()) else {
            fail!("{} names no commit", file.display());
        };
        let steps = lines
            .next()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_string)
            .collect();
        Ok(Some(Built {
            commit: commit.to_string(),
            steps,
        }))
    }

    /// Written in one rename, so a reader never sees half of it.
    fn write(&self, file: &Path) -> Result<()> {
        crate::whole_file::write(file, format!("{}\n{}\n", self.commit, self.steps.join(" ")))
    }

    fn clear(file: &Path) -> Result<()> {
        match std::fs::remove_file(file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("removing {}: {error}", file.display())
            }
            _ => Ok(()),
        }
    }

    /// Whether a run of picks of <wanted> (named <asked> by the caller) may go
    /// ahead in <checkout>, which is on <head> and last built <built>.
    fn allows_class_run(
        built: Option<&Built>,
        checkout: &str,
        asked: &str,
        wanted: &str,
        head: &str,
    ) -> Result<()> {
        let hint = "name the steps that build it (a run of only --test or --shots flags tests what the checkout holds)";
        let Some(built) = built else {
            fail!("{checkout} has no finished build to test {asked} ({wanted}) against: {hint}");
        };
        if built.commit != wanted {
            fail!(
                "{checkout} last built {} (steps: {}), not {asked} ({wanted}): {hint}",
                built.commit,
                built.steps.join(" ")
            );
        }
        if head != wanted {
            fail!("{checkout} built {wanted} but is now on {head}: {hint}");
        }
        Ok(())
    }
}

/// How a lane's steps ended, when none failed because of the code.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    Finished,
    /// A takeover stopped a step.
    Stopped,
    /// A step failed after printing a line that says the host failed it
    /// (`HOST_FAULTS`): what the host refuses from now on, and the line.
    HostFault(Refused, String),
}

/// The lines a mac lane step prints when the host failed it, not the code,
/// each with what the host refuses once a step failed after printing it.
const HOST_FAULTS: [(&str, Refused); 2] = [
    (crate::xcode::CANNOT_START, Refused::Xcode),
    (
        crate::ui_test_settings::AUTOMATION_TIMED_OUT,
        Refused::UiAutomation,
    ),
];

/// The texts of `HOST_FAULTS`, which a mac lane step's run watches for.
const HOST_FAULT_TEXTS: &[&str] = &[HOST_FAULTS[0].0, HOST_FAULTS[1].0];

/// What the host refuses after a step failed having printed <said>, a line
/// that holds one of `HOST_FAULT_TEXTS`.
fn refused_after(said: &str) -> Refused {
    HOST_FAULTS
        .iter()
        .find(|(text, _)| said.contains(text))
        .map(|(_, refused)| *refused)
        .expect("the watch keeps only a line that holds one of the texts")
}

/// What a step's run keeps of its output: all of it for a shots run, whose
/// findings the shots cache holds (<all>), the first line saying the host
/// failed the step when the lane watches for one (<fault>), and each line
/// for <read>.
fn keep_for<'a>(
    kind: StepKind,
    all: &'a lock::Captured,
    fault: Option<&'a Found>,
    read: &'a lock::EachLine,
) -> Keep<'a> {
    Keep {
        all: (kind == StepKind::Shots).then_some(all),
        line_with: fault.map(|found| (HOST_FAULT_TEXTS, found)),
        each_line: Some(read),
    }
}

/// Runs <steps> in the checkout <top>, as `run_steps_with` does with <watch>.
fn run_steps(
    top: &Path,
    held: &lock::Held,
    steps: &[Step],
    log: &mut Vec<StepRecord>,
    shots: &ShotsCache,
    xcode_settings: &Path,
    watch: bool,
) -> Result<Ended> {
    run_steps_with(
        held,
        steps,
        log,
        watch,
        &mut LocalSteps {
            top,
            checkout: held.checkout_lock(),
            shots,
            xcode_settings,
        },
    )
}

/// Where a lane's steps run, and what a step's run needs from the checkout it
/// runs in.
trait StepRunner {
    /// The shots cache the runner's shots steps read and write.
    fn shots(&self) -> &ShotsCache;

    /// The number of scenes <count> prints when it runs in the checkout.
    fn scenes(&mut self, count: &[String]) -> Result<usize>;

    /// Runs <step> (of <kind>) for at most <limit>, watching for a line of
    /// `HOST_FAULTS` when given where to keep it, and giving each line of its
    /// output to the reader.
    fn run(
        &mut self,
        step: &Step,
        limit: Duration,
        kind: StepKind,
        fault: Option<&Found>,
        read: &lock::EachLine,
    ) -> Result<Within>;
}

/// The steps that run in the checkout <top> here, with the Xcode settings file
/// that the checkout needs (xcode_cache.rs).
struct LocalSteps<'a> {
    top: &'a Path,
    checkout: CheckoutLock<'a>,
    shots: &'a ShotsCache,
    xcode_settings: &'a Path,
}

impl StepRunner for LocalSteps<'_> {
    fn shots(&self) -> &ShotsCache {
        self.shots
    }

    fn scenes(&mut self, count: &[String]) -> Result<usize> {
        count_scenes(self.top, count, self.checkout)
    }

    /// A shots run compares with its before run, read where the cache holds
    /// it, takes the board pictures earlier runs drew from the cache's picture
    /// folder, and caches the findings it printed. While the shots program
    /// runs, the step holds the shared lock that keeps the prune from deleting
    /// what it reads or writes in the folder `shots` (shots_cache.rs).
    fn run(
        &mut self,
        step: &Step,
        limit: Duration,
        kind: StepKind,
        fault: Option<&Found>,
        read: &lock::EachLine,
    ) -> Result<Within> {
        let (top, shots) = (self.top, self.shots);
        let kept = lock::Captured::default();
        let keep = keep_for(kind, &kept, fault, read);
        let (command, _writing) = if takes_pictures(step) {
            (shots.local_command(step), Some(shots.writing()?))
        } else {
            (step.command.clone(), None)
        };
        let outcome = run_command_keeping(
            top,
            &command,
            Some(limit),
            keep,
            self.xcode_settings,
            self.checkout,
        );
        if kind == StepKind::Shots {
            shots.cache_of(&outcome, &kept, step, &mut LocalRun(top));
        }
        outcome
    }
}

/// The steps that run in a checkout on a host, under the build's programs.
struct RemoteSteps<'a, 'checkout> {
    remote: &'a mut RemoteApp<'checkout>,
    with: &'a StepsWith<'a>,
}

impl StepRunner for RemoteSteps<'_, '_> {
    fn shots(&self) -> &ShotsCache {
        self.with.shots
    }

    fn scenes(&mut self, count: &[String]) -> Result<usize> {
        let held = self.remote.with_programs(self.with.programs, count)?;
        let said = self.remote.run_in(
            &crate::app_remote::shell_line(&held),
            "counting a run's scenes",
        )?;
        parse_scene_count(&said)
    }

    fn run(
        &mut self,
        step: &Step,
        limit: Duration,
        kind: StepKind,
        fault: Option<&Found>,
        read: &lock::EachLine,
    ) -> Result<Within> {
        run_remote_step(self.remote, step, limit, kind, self.with, fault, read)
    }
}

/// Runs <steps> in order, each by <run>, stopping at the first that fails,
/// except a shots run whose output names the pairs that fail it: that run
/// did its work, so the steps after it run, and once they pass the run fails
/// with the first such run's status.
/// `run_steps_with` records each step as it starts in the record <held>
/// keeps beside its lock (holder_run.rs), and times each step it starts into
/// <log> (app_timing.rs), with what a step that failed or ran past its limit
/// said (app_build_step_failure.rs). With <watch>, each step's run watches for a line of
/// `HOST_FAULTS`, and a step that fails after printing one ends the run with
/// `HostFault`. Once this process got a stop signal, it starts no step and
/// fails, naming the signal; the step the signal ended is recorded as
/// stopped.
fn run_steps_with(
    held: &lock::Held,
    steps: &[Step],
    log: &mut Vec<StepRecord>,
    watch: bool,
    runner: &mut dyn StepRunner,
) -> Result<Ended> {
    // The status of the first shots run whose pairs failed it.
    let mut pairs_failed = None;
    // The lane's capture step ran: it counted the screenshots, so the shots
    // runs after it only read them from the picture folder.
    let mut capture_ran = false;
    for step in steps {
        let _window = if let Some(access) = step.window {
            match held.take_window(&step.name, access)? {
                Some(window) => Some(window),
                None => return Ok(Ended::Stopped),
            }
        } else {
            None
        };
        lock::refuse_when_stopped(&format!("was to start step {}", step.name))?;
        note(format!("step {}", step.name));
        held.runs(&step.name)?;
        let kind = match pick_kind_of(&step.name) {
            Some(PickKind::Class) => StepKind::Test,
            Some(PickKind::Shots) => StepKind::Shots,
            None => StepKind::Step,
        };
        // A run's limit grows with its work, so a long run is not a stuck one.
        let mut scene_count = None;
        let limit = match &step.scenes {
            Some(scenes) => {
                let count = runner.scenes(&scenes.count)?;
                scene_count = Some(count);
                let limit = scenes.limit_for(step.limit(), count)?;
                note(format!(
                    "step {} takes {count} scenes, so its limit is {}",
                    step.name,
                    show_duration(limit)
                ));
                limit
            }
            None => step.limit(),
        };
        let timer = Timer::start();
        let found = Found::default();
        let read = StepRead::default();
        let reused = ReusedRead::default();
        let (failures, pictures) = (read.each_line(), reused.each_line());
        let each: lock::EachLine = Arc::new(move |text: &str| {
            failures(text);
            pictures(text);
        });
        let outcome = runner.run(step, limit, kind, watch.then_some(&found), &each);
        let failed_pairs = kind == StepKind::Shots && !read.failure().failed.is_empty();
        let stopped_by = lock::stop_signal();
        let result = match &outcome {
            Ok(Within::Ran(Ran::Finished)) => StepResult::Passed,
            _ if stopped_by.is_some() => StepResult::Stopped,
            Ok(Within::Ran(Ran::Stopped)) => StepResult::TakenOver,
            Ok(Within::TimedOut) => StepResult::TimedOut(read.failure()),
            Err(_) => StepResult::Failed(read.failure()),
        };
        let mut record = timer.finish(&step.name, kind, result);
        let captures = is_capture(&step.name);
        if kind == StepKind::Shots || captures {
            // A step that ended before its run finished counted no
            // screenshots, and neither does a shots run after the capture step.
            let finished = matches!(record.result, StepResult::Passed) || failed_pairs;
            let counted = reused.reused();
            let pictures = match (&counted, finished && !capture_ran) {
                (Ok(reused), true) => Pictures::of(scene_count, *reused),
                _ => None,
            };
            capture_ran |= captures;
            record.shots = Some(runner.shots().use_of(step, pictures));
            if let Err(why) = counted {
                log.push(record);
                fail!("{why}");
            }
        }
        log.push(record);
        if let Some(signal) = stopped_by {
            fail!(
                "the build stopped on {} during step {}",
                lock::signal_name(signal),
                step.name
            );
        }
        match outcome {
            Ok(Within::Ran(Ran::Finished)) => {}
            Ok(Within::Ran(Ran::Stopped)) => return Ok(Ended::Stopped),
            Ok(Within::TimedOut) => {
                let hint = if pick_kind_of(&step.name).is_some() {
                    String::new()
                } else {
                    format!(
                        " (a `limit {} <time>` line in {CONFIG} sets another)",
                        step.name
                    )
                };
                fail!(
                    "step {} ran past its limit of {} and its process group was killed; the lock goes to the next build{hint}",
                    step.name,
                    show_duration(limit),
                );
            }
            Err(Fail::Exit(code)) => {
                note(format!("step {} failed (exit {code})", step.name));
                if let Some(said) = found.lock().expect("no thread panicked holding it").take() {
                    return Ok(Ended::HostFault(refused_after(&said), said));
                }
                if !failed_pairs {
                    return Err(Fail::Exit(code));
                }
                note(format!(
                    "{} cached its findings, so the lane runs its other steps",
                    step.name
                ));
                pairs_failed = pairs_failed.or(Some(code));
            }
            Err(other) => return Err(other),
        }
    }
    match pairs_failed {
        Some(code) => Err(Fail::Exit(code)),
        None => Ok(Ended::Finished),
    }
}

/// The repo's `.app-build` at <commit>, read without touching a checkout's
/// tree, to tell which lanes the steps are in before any lock is taken. A
/// lane's own run reads it again from its checkout, under its lock.
pub fn config_at(places: &Places, repo: &str, commit: &str) -> Result<Config> {
    let source = fetched_local_source(places, repo)?;
    config_in(&source, commit)
}

/// What a lane's build does in its app checkout (app_site.rs).
impl Site<'_> {
    /// The commit <commit> names, and the commit the checkout is on now.
    fn resolved_and_head(
        &mut self,
        places: &Places,
        repo: &str,
        commit: &str,
    ) -> Result<(String, String)> {
        let source = self.fetched_source(places, repo)?;
        let sha = match self {
            Site::Local(_, _, checkout) => resolve_checkout(&source, commit, *checkout)?,
            Site::Remote(_) => resolve(&source, commit)?,
        };
        Ok((sha, self.head()?))
    }

    /// The name of the host the checkout is on.
    fn host_name(&self) -> &str {
        match self {
            Site::Local(..) => crate::host::LOCAL,
            Site::Remote(remote) => &remote.host.name,
        }
    }

    /// The cores and load of the checkout's host now; None when the host does
    /// not answer, and the function says so on stderr.
    fn load(&self, places: &Places, ssh: &Ssh) -> Option<crate::host::Load> {
        match self {
            Site::Local(..) => Some(crate::host::Load::here()),
            Site::Remote(remote) => match crate::host::probe(places, ssh, &remote.host) {
                Ok(probe) => Some(probe.load()),
                Err(error) => {
                    note(format!(
                        "host {} did not say its load, so this run's record has none: {error}",
                        remote.host.name
                    ));
                    None
                }
            },
        }
    }

    /// Switches the checkout to <commit> (which must have no changes) and
    /// reads the repo's `.app-build` there.
    pub fn check_out(&mut self, places: &Places, repo: &str, commit: &str) -> Result<CheckedOut> {
        match self {
            Site::Local(top, _, checkout) => check_out(top, commit, *checkout),
            Site::Remote(remote) => {
                let source = fetched_local_source(places, repo)?;
                let sha = resolve(&source, commit)?;
                remote.switch_to(&source, &sha)?;
                let config = config_in(&source, &sha)?;
                Ok(CheckedOut { sha, config })
            }
        }
    }

    /// Runs <steps> in the checkout with what <with> gives them, as
    /// `run_steps_with` does with its `watch`.
    fn run_steps(
        &mut self,
        held: &lock::Held,
        steps: &[Step],
        with: &StepsWith,
        log: &mut Vec<StepRecord>,
    ) -> Result<Ended> {
        match self {
            Site::Local(top, settings, _) => {
                run_steps(top, held, steps, log, with.shots, settings, with.watch)
            }
            Site::Remote(remote) => run_steps_with(
                held,
                steps,
                log,
                with.watch,
                &mut RemoteSteps { remote, with },
            ),
        }
    }
}

/// What a lane's steps run with.
struct StepsWith<'a> {
    /// The folders a step on a host copies back here after it runs.
    results: &'a [String],
    /// Where a shots run caches its findings here.
    shots: &'a ShotsCache,
    /// The build's programs, which a step on a host runs under, copied there
    /// (programs.rs).
    programs: &'a Pinned,
    /// Whether each step's run watches for a line of `HOST_FAULTS`.
    watch: bool,
}

/// Runs <step> (of <kind>) in the checkout on a host under the build's
/// programs there, watching for a line of `HOST_FAULTS` in <fault> and
/// giving each line to <read>; <with> gives what it runs with. A shots run
/// compares with its before run, which the run copies to a temporary
/// file on the host and deletes after, and keeps its findings here. Then
/// copies <results> back, since a step that failed or ran out of time has
/// written the reports its reader needs most. A copy that fails fails a step
/// that passed; after a step that failed, the run notes the copy's error and
/// keeps the step's own result.
fn run_remote_step(
    remote: &mut RemoteApp,
    step: &Step,
    limit: Duration,
    kind: StepKind,
    with: &StepsWith,
    fault: Option<&Found>,
    read: &lock::EachLine,
) -> Result<Within> {
    let StepsWith { results, shots, .. } = *with;
    let store = takes_pictures(step).then_some(shots);
    // Held while the copies read the before run and the picture folder,
    // which the prune then leaves.
    let reading = store.map(|store| store.writing()).transpose()?;
    let before = store.and_then(|store| store.before_of(step));
    let placed = match before {
        Some(Compared::With(findings)) => Some(remote.put_temporary(findings)?),
        _ => None,
    };
    let pictures = match store {
        Some(store) => Some(remote.put_pictures(&store.pictures(), &store.tree_part())?),
        None => None,
    };
    drop(reading);
    let command = match (store, &pictures) {
        (Some(store), Some(there)) => store.command(
            step,
            before.map(|before| before.arguments(placed.as_deref())),
            there,
        ),
        _ => step.command.clone(),
    };
    let mut held = remote.with_programs(with.programs, &command)?;
    // A step that opens windows holds the host's display on while it runs.
    if step.window.is_some() {
        held = remote.keeping_display_on(&held);
    }
    let kept = lock::Captured::default();
    let keep = keep_for(kind, &kept, fault, read);
    let ran = remote.run_step(&held, &command[0], Some(limit), keep);
    if let Some(there) = &placed
        && let Err(error) = remote.remove_temporary(there)
    {
        note(error.text());
    }
    if let (Some(store), Some(there)) = (store, &pictures) {
        let brought = store.bring_back_pictures(remote, there);
        // The capture step's result is the pictures it brings back: a copy
        // that fails fails a capture that finished, as a failed copy of
        // `results` fails a step that passed.
        if is_capture(&step.name)
            && matches!(ran, Ok(Within::Ran(Ran::Finished)))
            && let Err(error) = brought
        {
            return Err(error);
        }
        if kind == StepKind::Shots {
            store.cache_of(&ran, &kept, step, remote);
        }
    }
    if results.is_empty() || matches!(ran, Ok(Within::Ran(Ran::Stopped))) {
        return ran;
    }
    match (remote.pull(results), &ran) {
        (Ok(()), _) => ran,
        (Err(error), Ok(Within::Ran(Ran::Finished))) => Err(error),
        (Err(error), _) => {
            note(error.text());
            ran
        }
    }
}

/// The full id of <git_ref> and the repo's `.app-build` at it, read without
/// touching a checkout's tree, with the app checkout's folder they were read
/// from.
pub fn commit_and_config(
    places: &Places,
    repo: &str,
    git_ref: &str,
) -> Result<(std::path::PathBuf, String, Config)> {
    let top = fetched_local_source(places, repo)?;
    let commit = resolve(&top, git_ref)?;
    let config = config_in(&top, &commit)?;
    Ok((top, commit, config))
}

/// The first 12 characters of a commit.
fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// The prefix of the shots run <step>, which `lane_steps` names `shots
/// <prefix>`.
fn shots_prefix(step: &Step) -> &str {
    PickKind::Shots
        .argument_of_step(&step.name)
        .unwrap_or(&step.name)
}

/// What a lane's process is given first, after the commit to build, with the
/// ref the caller named (a branch name, or the commit when the caller named
/// one): the run records it as the ref it built (`app_timing::RunRecord`),
/// which is how `builds land` finds a branch's builds.
const BUILT_REF: &str = "--built-ref";

/// The flag that names the project whose store records a build of a ref that
/// no claimed worktree speaks for (`app_timing::recording_project`). A lane's
/// process is given the project its request chose.
pub const PROJECT: &str = "--project";

/// What a lane's process is given first when its steps are only what its
/// shots picks need, because no path the branch changes needs a step in the
/// lane (derive.rs): the lane then builds nothing when the picture folder
/// caches every shot of its picks for the lane's app tree
/// (`capture_query`), and takes the shots from the folder.
const NEEDS_ONLY: &str = "--needs-only";

/// What a lane's process is given first when the run has `--fresh`: its shots
/// runs take every shot again and replace the pictures in the picture folder.
const FRESH: &str = "--fresh";

/// The id that keys <lane>'s shots in the picture folder at <commit> of the
/// repo at <source>, whose `.app-build` there is <config>, when they are drawn
/// in <environment>: the lane's app source tree combined with the environment
/// (app_tree.rs `in_environment`).
fn picture_tree(
    source: &Path,
    commit: &str,
    config: &Config,
    lane: Lane,
    environment: &str,
) -> Result<String> {
    Ok(crate::app_tree::in_environment(
        &crate::app_tree::of(source, commit, config, lane)?,
        environment,
    ))
}

/// The marks a lane needs to run its shots without building the app.
fn capture_query(
    config: &Config,
    lane: Lane,
    prefixes: &[shots::Prefix],
) -> Result<shots_cache::CaptureQuery> {
    let Some(picker) = config.picker(lane, PickKind::Shots) else {
        let first = prefixes
            .first()
            .expect("a lane has shots picks when it asks whether the folder caches them");
        return Err(missing_line(lane, PickKind::Shots, first.as_str()));
    };
    let windows = picker.command.iter().any(|word| word == "--windows");
    Ok(shots_cache::CaptureQuery {
        prefixes: prefixes.to_vec(),
        windows,
    })
}

/// What a shots run of a prefix compares with.
#[derive(Debug, PartialEq, Eq)]
enum Compared {
    /// The findings.json of the run of the merge base.
    With(std::path::PathBuf),
    /// Nothing: the merge base (this commit) has no scene under the prefix,
    /// so every scene the run selects is new (`builds shots run
    /// --empty-before`).
    Nothing(String),
}

impl Compared {
    /// The arguments of `builds shots run` that name it. A run on a host
    /// reads the findings from the copy at <placed>.
    fn arguments(&self, placed: Option<&str>) -> [String; 2] {
        match self {
            Compared::With(file) => [
                "--before".to_string(),
                placed.map_or_else(|| file.display().to_string(), str::to_string),
            ],
            Compared::Nothing(base) => ["--empty-before".to_string(), base.clone()],
        }
    }
}

/// <command>, a run of a `shots` line, with the <before> arguments that name
/// what it compares with.
fn with_before(command: &[String], before: [String; 2]) -> Vec<String> {
    let mut command = command.to_vec();
    command.extend(before);
    command
}

/// Where a lane's run caches its `--shots` runs (shots_cache.rs), and the cached
/// runs they compare with.
struct ShotsCache {
    places: Places,
    repo: String,
    lane: Lane,
    /// The versions of the shots checker the store keys the runs on: the ones
    /// this builds is built with.
    versions: crate::shots_program::Versions,
    /// What each shots prefix of the lane compares with, by its prefix.
    befores: Vec<(String, Compared)>,
    /// The host software that draws the lane's shots (app_platform.rs),
    /// which keys the runs this store caches and compares with.
    environment: String,
    /// The id that keys the shots the app takes in the picture folder: the
    /// lane's app source tree at the commit (app_tree.rs) combined with the
    /// environment (`app_tree::in_environment`).
    app_tree: String,
    /// The lane built no app, so a shots run fails instead of capturing
    /// (`builds shots run --no-capture`).
    no_capture: bool,
    /// The run has `--fresh`: its shots runs take every selected shot again
    /// and replace the folder's pictures (`builds shots run --retake`).
    retake: bool,
    /// The lane runs the merge base of a build to make the run that build
    /// compares with: its shots runs pass `--base`.
    base_run: bool,
    /// Whether the run each shots prefix compares with was saved, built in
    /// this build's turn, or not needed.
    bases: Vec<(String, BaseRun)>,
}

/// The names of the shots <lane> declares at <base> in <source>, read once
/// into <cache> and then reused for every prefix of a run.
fn base_declared<'a>(
    cache: &'a mut Option<Vec<String>>,
    source: &Path,
    base: &str,
    lane: Lane,
) -> Result<&'a [String]> {
    let names = match cache.take() {
        Some(names) => names,
        None => crate::shots_declared::declared_names_in_lane(source, base, lane)?,
    };
    Ok(cache.insert(names))
}

impl ShotsCache {
    /// The store for <repo>'s <lane> run of the full commit <commit>, whose
    /// commits <source> holds, with the before run of each shots pick of
    /// <picks> in the lane: the run of <commit>'s merge base with origin/main
    /// made in <environment> (shots_cache.rs). <app_tree> is the picture
    /// folder's id for the lane's shots at <commit> (`picture_tree`). A commit on origin/main is its own merge base and has
    /// none, and neither has a pick that starts no shot declared at the merge
    /// base. Fails when the store holds no before run for another pick,
    /// naming the command that makes one: a lane's build makes the run of
    /// the merge base first (`ensure_base`), so only a lane that runs the
    /// picks against the checkout as it is reaches that. <built> holds the
    /// prefixes whose merge-base run the build made in its own turn
    /// (`ensure_base`).
    #[allow(clippy::too_many_arguments)]
    fn of(
        places: &Places,
        source: &Path,
        repo: &str,
        lane: Lane,
        environment: &str,
        commit: &str,
        app_tree: String,
        picks: &[Pick],
        no_capture: bool,
        retake: bool,
        base_run: bool,
        built: &[(shots::Prefix, MissCause)],
        program: &Path,
    ) -> Result<ShotsCache> {
        let versions = &crate::shots_program::VERSIONS;
        let mut befores = Vec::new();
        let mut bases = Vec::new();
        let mut base_declared_names = None;
        for selected in Pick::prefixes(picks) {
            let prefix = selected.as_str();
            let base = crate::derive::merge_base(source, commit)?;
            if base == commit {
                bases.push((prefix.to_string(), BaseRun::NotNeeded));
                continue;
            }
            match crate::shots_compare_again::before_run(
                places,
                repo,
                &base,
                lane,
                environment,
                prefix,
                program,
            )? {
                Some(before) => {
                    let how = match built.iter().find(|(prefix, _)| prefix == selected) {
                        Some((_, cause)) => BaseRun::Built(*cause),
                        None => BaseRun::Saved,
                    };
                    bases.push((prefix.to_string(), how));
                    befores.push((prefix.to_string(), Compared::With(before.findings_json())))
                }
                // A prefix that starts no shot declared at the merge base has
                // nothing to compare with: its scenes are all new.
                None if !crate::shots_declared::starts_a_shot(
                    base_declared(&mut base_declared_names, source, &base, lane)?,
                    prefix,
                ) =>
                {
                    bases.push((prefix.to_string(), BaseRun::NotNeeded));
                    befores.push((prefix.to_string(), Compared::Nothing(base.clone())));
                    note(format!(
                        "{prefix} starts no shot declared at the merge base {}, so its run compares with an empty run and every scene it selects is new",
                        short(&base)
                    ))
                }
                None => fail!(
                    "builds holds no run of {prefix} on {} for {}, the merge base of {} with origin/main, made in environment {environment} by shots checker {} (comparison) over {} (capture), so the run has nothing to compare with; `{}` makes one",
                    lane.word(),
                    short(&base),
                    short(commit),
                    versions.comparison,
                    versions.capture,
                    shot_findings::how_to_make(repo, &base, lane, prefix)
                ),
            }
        }
        Ok(ShotsCache {
            places: places.clone(),
            repo: repo.to_string(),
            lane,
            versions: crate::shots_program::Versions::clone(versions),
            befores,
            environment: environment.to_string(),
            app_tree,
            no_capture,
            retake,
            base_run,
            bases,
        })
    }

    /// What the shots run or capture step <step> used: the environment it ran in, whether the
    /// run it compares with was saved, and the screenshots it took
    /// (<pictures>).
    fn use_of(&self, step: &Step, pictures: Option<Pictures>) -> ShotsUse {
        // The capture step compares with nothing.
        let base_run = if is_capture(&step.name) {
            BaseRun::NotNeeded
        } else {
            let prefix = shots_prefix(step);
            self.bases
                .iter()
                .find(|(own, _)| own == prefix)
                .map(|(_, how)| *how)
                .expect("a lane's shots store holds the base run of each of its shots steps")
        };
        ShotsUse {
            environment: self.environment.clone(),
            base_run,
            pictures,
        }
    }

    /// The shared lock a shots step holds while it reads a before run or the
    /// picture folder, or writes to the folder (shots_cache.rs `writing`).
    fn writing(&self) -> Result<std::fs::File> {
        shots_cache::writing(&self.places)
    }

    /// The command of the shots run <step> in a checkout on this Mac: its
    /// line, comparing with its before run when its prefix has one, and
    /// taking the board pictures earlier runs drew and the shots earlier
    /// runs of the lane's app tree took from the picture folder
    /// (`builds shots run --pictures --app-tree`).
    fn local_command(&self, step: &Step) -> Vec<String> {
        let before = self.before_of(step).map(|before| before.arguments(None));
        let pictures = self.pictures().display().to_string();
        self.command(step, before, &pictures)
    }

    /// The command of the shots run <step>: its line, comparing with what
    /// <before> names when there is one, with the picture folder at
    /// <pictures> and the lane's app tree.
    fn command(&self, step: &Step, before: Option<[String; 2]>, pictures: &str) -> Vec<String> {
        let mut command = match before {
            Some(before) => with_before(&step.command, before),
            None => step.command.clone(),
        };
        command.extend([
            "--pictures".to_string(),
            pictures.to_string(),
            "--app-tree".to_string(),
            self.app_tree.clone(),
        ]);
        // A run that follows the lane's capture step already carries
        // `--no-capture` and takes nothing again; the capture step takes the
        // shots of the merge base without `--base`, which its prefixes pass.
        let after_capture = step.command.iter().any(|word| word == NO_CAPTURE);
        if self.no_capture && !after_capture {
            command.push(NO_CAPTURE.to_string());
        }
        if self.retake && !after_capture {
            command.push("--retake".to_string());
        }
        if self.base_run && !is_capture(&step.name) {
            command.push("--base".to_string());
        }
        command
    }

    /// The repo's picture folder on this Mac.
    fn pictures(&self) -> std::path::PathBuf {
        shots_cache::pictures(&self.places, &self.repo)
    }

    /// The part of the picture folder besides the board pictures that a run
    /// on a host reads: the shots cached for the lane's app tree.
    fn tree_part(&self) -> String {
        format!("{}/{}", self.lane.word(), self.app_tree)
    }

    /// What the shots run <step> compares with, when its prefix has
    /// something.
    fn before_of(&self, step: &Step) -> Option<&Compared> {
        let prefix = shots_prefix(step);
        self.befores
            .iter()
            .find(|(own, _)| own == prefix)
            .map(|(_, compared)| compared)
    }

    /// Caches what the shots run <step> printed (<cached>), when it ended
    /// (<outcome>) with an exit status, as the run of the commit the site is
    /// on.
    fn cache_of(
        &self,
        outcome: &Result<Within>,
        kept: &lock::Captured,
        step: &Step,
        site: &mut dyn RunSite,
    ) {
        let exit = match outcome {
            Ok(Within::Ran(Ran::Finished)) => 0,
            Err(Fail::Exit(code)) => i64::from(*code),
            _ => return,
        };
        let text = String::from_utf8_lossy(&kept.lock().expect("no panic holding it")).into_owned();
        self.cache(site, shots_prefix(step), exit, &text);
    }

    /// Caches a run of <prefix> that exited <exit> and printed <output> as the
    /// run of the commit the site is on: what the shots program printed and
    /// the report it names, copied from the site (shot_findings.rs). A run
    /// whose report's findings.json names another version of the shots
    /// checker than the cache's is not cached, since no lookup of this builds would
    /// find it under its own version. A run that printed no findings, or a
    /// write or copy that fails, is said and changes nothing about the
    /// build's result; a report that fails to copy leaves the run cached
    /// without one.
    fn cache(&self, site: &mut dyn RunSite, prefix: &str, exit: i64, output: &str) {
        let findings = shot_findings::findings_of(output);
        if !shot_findings::has_findings(exit, findings) {
            note(format!(
                "the shots run of {prefix} exited {exit} without findings, so none were cached"
            ));
            return;
        }
        let staged = site.commit().and_then(|commit| {
            let key = shots_cache::Key::new(
                &self.repo,
                &commit,
                self.lane,
                &self.environment,
                prefix,
                &self.versions,
            );
            shots_cache::stage(&self.places, &key)
        });
        let staging = match staged {
            Ok(staging) => staging,
            Err(error) => {
                note(format!("the shots run was not cached: {}", error.text()));
                return;
            }
        };
        let report = staging.report();
        match shot_findings::report_dir_in(output) {
            Some(dir) => {
                if let Err(error) = site.fetch_report(&dir, &report) {
                    note(format!("the shots report was not cached: {}", error.text()));
                    if let Err(error) = staging.discard_report() {
                        note(error.text());
                        return;
                    }
                } else if let Err(why) = self.same_versions(&report) {
                    note(format!("the shots run was not cached: {why}"));
                    if let Err(error) = staging.discard() {
                        note(error.text());
                    }
                    return;
                }
            }
            None => note(format!(
                "the shots run of {prefix} printed no report path, so no report was cached"
            )),
        }
        match staging.finish(exit, findings) {
            Ok(cached) => shot_findings::note_report(&cached),
            Err(error) => note(format!("the shots run was not cached: {}", error.text())),
        }
    }

    /// Copies the pictures a run on the host drew or took into the picture
    /// folder <there> on the host into the repo's picture folder here, and
    /// deletes <there>. Says what fails and returns it: the caller decides
    /// whether the copy is the step's result.
    fn bring_back_pictures(&self, remote: &mut RemoteApp, there: &str) -> Result<()> {
        let pictures = self.pictures();
        let incoming = shots_cache::incoming(&pictures);
        // The lock keeps the prune from deleting the incoming folder while
        // the copy fills it.
        let brought = self.writing().and_then(|_writing| {
            remote.fetch_pictures(there, &incoming)?;
            shots_cache::merge_pictures(&incoming, &pictures)
        });
        let brought = brought.map_err(|error| {
            let said = format!(
                "the pictures of the run on host {} were not cached: {}",
                remote.host.name,
                error.text()
            );
            note(&said);
            Fail::Message(said)
        });
        if let Err(error) = remote.remove_temporary(there) {
            note(error.text());
        }
        brought
    }

    /// Whether the findings.json in the report folder <folder> names the
    /// store's versions of the shots checker; why not, when it names others.
    fn same_versions(&self, folder: &Path) -> std::result::Result<(), String> {
        let named = shot_findings::versions_in(folder).map_err(|error| error.text())?;
        if named == self.versions {
            return Ok(());
        }
        Err(format!(
            "the run's findings.json names shots checker {} over {}, and this builds caches the runs of shots checker {} over {}: the run's `builds shots` was another builds than this one",
            named.comparison, named.capture, self.versions.comparison, self.versions.capture
        ))
    }
}

/// Where a shots run ran: what `ShotsCache::cache` asks of it.
trait RunSite {
    /// The commit the checkout is on.
    fn commit(&mut self) -> Result<String>;
    /// Copies the report in the checkout's folder <dir> to <into>.
    fn fetch_report(&mut self, dir: &str, into: &Path) -> Result<()>;
}

/// An app checkout on this Mac.
struct LocalRun<'a>(&'a Path);

impl RunSite for LocalRun<'_> {
    fn commit(&mut self) -> Result<String> {
        resolve(self.0, "HEAD")
    }
    fn fetch_report(&mut self, dir: &str, into: &Path) -> Result<()> {
        shot_findings::copy_report(Path::new(dir), into)
    }
}

impl RunSite for RemoteApp<'_> {
    fn commit(&mut self) -> Result<String> {
        self.head()
    }
    fn fetch_report(&mut self, dir: &str, into: &Path) -> Result<()> {
        RemoteApp::fetch_report(self, dir, into)
    }
}

/// What a lane's run has timed so far, for `record` to write when it ends.
struct Timing {
    /// The lane's lock as the run last held it; None until the run takes
    /// one.
    held: Option<HeldTimes>,
    steps: Vec<StepRecord>,
    /// The commit the run checked out to build; None until it checks one out.
    commit: Option<String>,
}

/// The queue wait and checkout `lock::Held` records, with the checkout
/// host's load when the run took it and the run's resolved target commit.
struct HeldTimes {
    joined_at: i64,
    wait: Duration,
    checkout: String,
    load: Option<crate::host::Load>,
    requested_commit: String,
}

impl HeldTimes {
    fn of(held: &lock::Held, commit: &str, load: Option<crate::host::Load>) -> HeldTimes {
        HeldTimes {
            joined_at: held.joined_at,
            wait: held.waited,
            checkout: held.checkout.clone(),
            load,
            requested_commit: commit.to_string(),
        }
    }
}

impl Timing {
    fn start() -> Timing {
        Timing {
            held: None,
            steps: Vec::new(),
            commit: None,
        }
    }

    /// Writes the run of <request>, which ended with <outcome>, to the store
    /// of the request's project when it took a checkout. When an install moved the store to
    /// another schema while the run built, the run goes to the store through
    /// the installed builds (handed_runs.rs). A repo in no project has no
    /// store to write to.
    fn record(
        self,
        places: &Places,
        repo: &str,
        lane: Lane,
        request: LaneRequest,
        outcome: RunEnd,
    ) -> Result<()> {
        let LaneRequest {
            of, reused_shots, ..
        } = request;
        let BuildOf {
            named: built_ref,
            project,
            ..
        } = of;
        let Some(held) = self.held else {
            return Ok(());
        };
        let run = RunRecord {
            started_at: held.joined_at,
            repo: repo.to_string(),
            lane,
            checkout: held.checkout,
            built_ref: built_ref.to_string(),
            requested_commit: Some(held.requested_commit),
            built_commit: self.commit.map(|sha| BuiltCommit {
                reused_shots: reused_shots.to_vec(),
                ..sha.into()
            }),
            who: who_asked(),
            wait_ms: held.wait.as_millis() as u64,
            outcome,
            load: held.load,
            steps: self.steps,
        };
        crate::handed_runs::record(places, &run, project).context(|| {
            format!(
                "recording the {} app build of {repo} {built_ref}",
                lane.word()
            )
        })
    }
}

/// The claim of the worktree this process runs in: who asked for the build.
fn who_asked() -> Option<String> {
    let checkout = Checkout::at(Path::new(".")).ok()??;
    let claim = Claim::read(&checkout.claim_file()).ok()??;
    Some(claim.who).filter(|who| !who.is_empty())
}

#[derive(Debug)]
enum LaneError {
    Build(Fail),
    Pictures(shots_cache::PictureLeaseError),
}

type LaneResult<T> = std::result::Result<T, LaneError>;

impl From<Fail> for LaneError {
    fn from(error: Fail) -> Self {
        Self::Build(error)
    }
}

impl From<shots_cache::PictureLeaseError> for LaneError {
    fn from(error: shots_cache::PictureLeaseError) -> Self {
        Self::Pictures(error)
    }
}

impl From<LaneError> for Fail {
    fn from(error: LaneError) -> Self {
        match error {
            LaneError::Build(error) => error,
            LaneError::Pictures(error) => error.into(),
        }
    }
}

/// Builds what <request> asks for in <lane>'s app checkout on the lane's
/// lock, and times the run and its steps and records them (app_timing.rs). A
/// lane that adds a checkout on a host keeps the host's free disk above
/// the floor of <tool>'s rules. A stop signal ends the lane from the moment
/// it starts, and a run that one reached before it passed is recorded as
/// stopped. <programs> are the build's programs, which the lane copies to
/// each host it runs a step on.
fn run_lane(
    tool: &Tool,
    repo: &str,
    lane: Lane,
    request: LaneRequest,
    programs: &Pinned,
) -> Result<()> {
    lock::end_on_stop_signals();
    let mut timing = Timing::start();
    let result = build_lane(tool, repo, lane, request, programs, &mut timing);
    let outcome = match &result {
        Ok(()) => RunEnd::Passed,
        Err(_) if lock::stop_signal().is_some() => RunEnd::Stopped,
        Err(_) => RunEnd::Failed,
    };
    let recorded = timing.record(&tool.places, repo, lane, request, outcome);
    crate::fail::with_cleanup(result.map_err(Fail::from), recorded)
}

/// What a caller asked one lane to build: <of>, with the lane's steps
/// <names> (all of them when none and no <picks>), then the test classes and
/// shots of <picks>.
#[derive(Clone, Copy)]
struct LaneRequest<'a> {
    of: BuildOf<'a>,
    names: &'a [String],
    picks: &'a [Pick],
    reused_shots: &'a [shots::Prefix],
    /// The steps <names> are only what the shots picks need (`NEEDS_ONLY`).
    needs_only: bool,
    /// The run has `--fresh` (`FRESH`).
    fresh: bool,
}

/// The merge base of a lane's commit with origin/main, where the lane makes
/// the runs that the shots of the commit compare with, read from git alone,
/// before the lane takes a checkout.
struct MergeBase {
    commit: String,
    /// The repo's `.app-build` at the merge base.
    config: Config,
    /// The steps of the lane that a run of its picks at the merge base needs:
    /// the lane's `needs` steps.
    names: Vec<String>,
    /// The lane's shots picks that start a shot declared at the merge base.
    prefixes: Vec<shots::Prefix>,
    /// The prefixes of the picks that select a scene shown in its window at
    /// the merge base.
    windowed: Vec<(String, Access)>,
}

impl MergeBase {
    /// The merge base of <commit> when it is not <commit> itself and one of
    /// the lane's shots picks among <picks> starts a shot the lane declares
    /// there; None otherwise, since the merge base then has no run to make.
    fn of(lane: Lane, source: &Path, commit: &str, picks: &[Pick]) -> Result<Option<Self>> {
        let mut prefixes: Vec<_> = Pick::prefixes(picks).cloned().collect();
        if prefixes.is_empty() {
            return Ok(None);
        }
        let base = crate::derive::merge_base(source, commit)?;
        if base == commit {
            return Ok(None);
        }
        let declared = crate::shots_declared::declared_names_in_lane(source, &base, lane)?;
        prefixes.retain(|prefix| crate::shots_declared::starts_a_shot(&declared, prefix.as_str()));
        if prefixes.is_empty() {
            return Ok(None);
        }
        let config = config_in(source, &base)?;
        let commands = Self::commands(lane, &prefixes);
        let names = crate::derive::plan(&config, &[], &commands)
            .into_iter()
            .find(|(own_lane, _)| *own_lane == lane)
            .map(|(_, names)| names)
            .expect("derive::plan lists each lane that has picks");
        let windowed = crate::shots_declared::windowed_prefixes(source, &base, &config, &commands)?;
        Ok(Some(MergeBase {
            commit: base,
            config,
            names,
            prefixes,
            windowed,
        }))
    }

    fn commands(lane: Lane, prefixes: &[shots::Prefix]) -> Vec<Pick> {
        prefixes
            .iter()
            .map(|prefix| Pick::Shots(lane, prefix.clone()))
            .collect()
    }

    /// The steps that a run of the picks at the merge base takes.
    fn steps(&self, lane: Lane) -> Result<Vec<Step>> {
        lane_steps(
            &self.config,
            lane,
            &self.names,
            &Self::commands(lane, &self.prefixes),
            &self.windowed,
            &|_| Ok(None),
        )
    }
}

/// Takes the exclusive lock on the run of <lane>'s shots at the merge base
/// <base> of <repo> in <environment> (a file in the repo's folder on the
/// volume), so that one lane's process at a time runs a base commit's shots in
/// an environment while the others wait for it and then find the runs cached.
/// A wait says so, and ends when the lock frees, or at once when a takeover
/// stops this build (None) or a stop signal reaches it (an error), as every
/// wait for a lock does (lock_wake.rs `wait_for_lock`). The lock lasts as long
/// as the file returned.
fn lock_base_run(
    places: &Places,
    repo: &str,
    lane: Lane,
    base: &str,
    environment: &str,
) -> Result<Option<std::fs::File>> {
    use std::os::fd::FromRawFd;
    let folder = places.root.join(repo);
    std::fs::create_dir_all(&folder).context(|| format!("creating {}", folder.display()))?;
    let path = folder.join(format!(
        "base-run-{}-{base}-{environment}.lock",
        lane.word()
    ));
    if let Some(file) = crate::evict::try_lock_creating(&path)? {
        return Ok(Some(file));
    }
    note(format!(
        "the run of the shots at the merge base {} is being made by another build: waiting for it",
        short(base)
    ));
    let waiting_for = format!("another build's run of the merge base {}", short(base));
    let taken = crate::lock_wake::wait_for_lock(&path, &waiting_for, &lock::takeover_stopped)?;
    // The descriptor is this process's, and closes when the file does.
    Ok(taken.map(|fd| unsafe { std::fs::File::from_raw_fd(fd) }))
}

/// What a lane's build holds while it works: the request, the repo's commits
/// and the programs the build runs under.
struct LaneBuild<'a> {
    places: &'a Places,
    repo: &'a str,
    lane: Lane,
    request: LaneRequest<'a>,
    programs: &'a Pinned,
    /// The folder on this Mac that holds the repo's commits.
    source: std::path::PathBuf,
    /// The repo's `.app-build` at the commit.
    config: Config,
    /// The prefixes of the lane's shots picks that select a scene shown in
    /// its window at the commit.
    windowed: Vec<(String, Access)>,
    /// The merge base, when the commit has one and the lane has shots picks
    /// that compare with a run of it (`MergeBase::of`). None also for a lane
    /// that runs its picks against the checkout as it is (no step named): it
    /// cannot switch the checkout to the merge base.
    base: Option<MergeBase>,
}

fn no_lane_steps(lane: Lane) -> Fail {
    Fail::Message(format!(
        "{CONFIG} names no step in the {} lane",
        lane.word()
    ))
}

fn build_lane(
    tool: &Tool,
    repo: &str,
    lane: Lane,
    request: LaneRequest,
    programs: &Pinned,
    timing: &mut Timing,
) -> LaneResult<()> {
    let places = &tool.places;
    let floor_kib = tool.rules.floor_kib;
    let (source, target, config) = commit_and_config(places, repo, request.of.commit)?;
    let commit = target.as_str();
    let request = LaneRequest {
        of: BuildOf {
            commit,
            ..request.of
        },
        ..request
    };
    let LaneRequest { picks, .. } = request;
    note(format!(
        "waiting for {repo}'s {} app lock, then building {commit}",
        lane.word()
    ));
    let ssh = Ssh::from_env();
    // Only a shots prefix that selects a scene shown in its window draws one.
    let windowed = crate::shots_declared::windowed_prefixes(&source, commit, &config, picks)?;
    let runs_checkout_as_it_is =
        request.names.is_empty() && !picks.is_empty() && !request.needs_only;
    let base = if runs_checkout_as_it_is {
        None
    } else {
        MergeBase::of(lane, &source, commit, picks)?
    };
    let build = LaneBuild {
        places,
        repo,
        lane,
        request,
        programs,
        source,
        config,
        windowed,
        base,
    };
    let mut steps = build.steps()?;
    if let Some(base) = &build.base {
        steps.extend(base.steps(lane)?);
    }
    // A step the repo keeps on this Mac (`here` lines) keeps the build off
    // the hosts' checkouts.
    let allowed = place_of(&steps);
    if allowed == lock::Where::Here {
        crate::worktree::ensure_local_app_checkout(tool, &ssh, repo, lane)?;
    }
    // A shots run that selects a scene shown in its window draws it on a
    // display of the host's.
    let draws = steps
        .iter()
        .any(|step| step.window.is_some() && takes_pictures(step));
    let mut held = if draws {
        lock::take_drawing(places, &ssh, repo, lane, allowed, floor_kib)?
    } else {
        lock::take_where(places, &ssh, repo, lane, allowed, floor_kib)?
    };
    loop {
        let mut site = Site::of(
            places,
            &ssh,
            repo,
            lane,
            &held.checkout,
            held.checkout_lock(),
        )?;
        timing.held = Some(HeldTimes::of(&held, commit, site.load(places, &ssh)));
        note(format!("building in {}", held.checkout));
        if std::env::var_os(PREFIXED).is_some() {
            // On both streams: each is read by a thread of its own, and a line
            // after the marker in a stream must find the new prefix.
            println!("{CHECKOUT_MARKER}{}", held.checkout);
            eprintln!("{CHECKOUT_MARKER}{}", held.checkout);
        }
        if let Site::Remote(remote) = &site {
            let folder = remote.local_folder();
            std::fs::create_dir_all(&folder)
                .context(|| format!("creating {}", folder.display()))?;
        }
        let built_file = places.app_built_file(repo, &held.checkout);
        match build.attempt(&mut site, &held, &built_file, timing)? {
            Ended::Finished => return Ok(()),
            Ended::Stopped => {
                note(format!(
                    "the build was stopped for a takeover; building {commit} again after it"
                ));
            }
            Ended::HostFault(refused, said) => {
                let host = site.host_name().to_string();
                crate::host::record_refusal(places, refused, &host, &said)?;
                note(format!(
                    "on host {host}, {}, so the build failed there and the code did not cause it. builds health now lists {host} with what fixes it, mac app builds skip its app checkouts, and {commit} builds again in a checkout on another host. The step printed: {said}",
                    refused
                        .mac_builds_cause()
                        .expect("after every host fault, the host refuses mac app builds")
                ));
            }
        }
        drop(site);
        held = held.requeue()?;
    }
}

/// What the lane runs at one commit it has checked out: a branch's commit, or
/// the merge base whose runs the branch's shots compare with.
struct CommitRun<'a> {
    /// The commit (full id) and its `.app-build`.
    sha: &'a str,
    config: &'a Config,
    /// The picture folder's id for the lane's shots at the commit
    /// (`picture_tree`).
    tree: String,
    /// The lane's steps to run, then the runs of <picks>.
    names: &'a [String],
    picks: &'a [Pick],
    /// The prefixes of <picks> that select a scene shown in its window.
    windowed: &'a [(String, Access)],
    environment: &'a str,
    /// The picture folder caches every shot of <picks> for <tree>: the lane
    /// builds nothing, and its shots runs capture none.
    cached_shots: bool,
    /// The runs take every shot again (`FRESH`).
    retake: bool,
    /// The runs are of the merge base, to be compared with: they pass
    /// `--base`.
    base_run: bool,
    /// The prefixes whose merge-base run this build made in its own turn.
    built: &'a [(shots::Prefix, MissCause)],
}

impl<'a> LaneBuild<'a> {
    /// The lane's steps at the commit, as `lane_steps` makes them from the
    /// commit's own `.app-build`.
    fn steps(&self) -> Result<Vec<Step>> {
        let commit = self.request.of.commit;
        let LaneRequest { names, picks, .. } = self.request;
        let program_of = |class: &str| {
            crate::derive::program_of(&self.source, &self.config, commit, self.lane, class)
        };
        lane_steps(
            &self.config,
            self.lane,
            names,
            picks,
            &self.windowed,
            &program_of,
        )
    }

    /// Builds the commit in <site>, whose lock <held> holds: first the run of
    /// the merge base that its shots compare with, when the cache lacks it,
    /// then the commit.
    fn attempt(
        &self,
        site: &mut Site,
        held: &lock::Held,
        built_file: &Path,
        timing: &mut Timing,
    ) -> LaneResult<Ended> {
        let LaneBuild {
            places, repo, lane, ..
        } = *self;
        let commit = self.request.of.commit;
        let LaneRequest {
            names,
            picks,
            needs_only,
            fresh,
            ..
        } = self.request;
        // The host software that draws the shots keys the runs and the
        // pictures this lane makes and reads, and is read from the host now.
        let environment = site.environment(lane)?;
        note(format!(
            "the shots of this build are drawn in environment {environment}"
        ));
        // A lane whose steps are only what its shots need, and whose shots
        // the picture folder caches, builds nothing.
        let tree = picture_tree(&self.source, commit, &self.config, lane, &environment)?;
        let prefixes: Vec<_> = Pick::prefixes(picks).cloned().collect();
        // A shots run draws the boards it compares with Chrome, so a build
        // with shots builds nothing on a host where Chrome does not start.
        if !prefixes.is_empty() {
            let host = site.host_name().to_string();
            let asking = std::cell::RefCell::new(&mut *site);
            let starts = crate::host_chrome::wait_until_starts(
                &host,
                &|| asking.borrow_mut().chrome_start(self.programs),
                &|words| note(words),
                &|step| match step {
                    Some(step) => held.runs(step),
                    None => held.runs_no_step(),
                },
                &lock::pause_before_trying_again,
            )?;
            if !starts {
                return Ok(Ended::Stopped);
            }
        }
        let pictures =
            (!prefixes.is_empty()).then(|| shots_cache::AppPictures::of(places, repo, lane, &tree));
        let run = |cached_shots| {
            let mut built = Vec::new();
            if let Some(base) = &self.base {
                let (ended, made) =
                    self.ensure_base(base, &environment, site, held, built_file, timing)?;
                if ended != Ended::Finished {
                    return Ok(ended);
                }
                built = made;
            }
            if cached_shots {
                note(
                    "the picture folder caches every shot these runs take for the lane's app tree, so the lane builds nothing and takes the shots from it",
                );
                Built::clear(built_file)?;
            } else if names.is_empty() && !picks.is_empty() {
                let (wanted, head) = site.resolved_and_head(places, repo, commit)?;
                Built::allows_class_run(
                    Built::read(built_file)?.as_ref(),
                    &held.checkout,
                    commit,
                    &wanted,
                    &head,
                )?;
            } else {
                // The switch and the steps change what the checkout holds.
                Built::clear(built_file)?;
            }
            let CheckedOut { sha, config } = site.check_out(places, repo, commit)?;
            timing.commit = Some(sha.clone());
            let run = CommitRun {
                sha: &sha,
                config: &config,
                tree,
                names,
                picks,
                windowed: &self.windowed,
                environment: &environment,
                cached_shots,
                retake: fresh,
                base_run: false,
                built: &built,
            };
            Ok(self.run_commit(site, held, built_file, timing, run)?)
        };
        match &pictures {
            Some(pictures) => {
                let query = (needs_only && !fresh)
                    .then(|| capture_query(&self.config, lane, &prefixes))
                    .transpose()?;
                pictures.while_held(query.as_ref(), run)?
            }
            None => run(false),
        }
    }

    /// Runs what <run> says in <site>, which is on the commit <run> names:
    /// the lane's steps for it, then its test and shots runs, with the shots
    /// cache of the commit's environment (`ShotsCache::of`).
    fn run_commit(
        &self,
        site: &mut Site,
        held: &lock::Held,
        built_file: &Path,
        timing: &mut Timing,
        run: CommitRun,
    ) -> Result<Ended> {
        let LaneBuild {
            places,
            repo,
            lane,
            programs,
            ..
        } = *self;
        let CommitRun {
            sha,
            config,
            tree,
            names,
            picks,
            windowed,
            environment,
            cached_shots,
            retake,
            base_run,
            built,
        } = run;
        let names: &[String] = if cached_shots { &[] } else { names };
        let program_of = |class: &str| {
            let program = crate::derive::program_of(&self.source, config, sha, lane, class)?;
            if let Some(program) = &program {
                note(format!(
                    "{class} is declared in the {program} sources, so the {} test line for {program} runs it",
                    lane.word()
                ));
            }
            Ok(program)
        };
        let steps = lane_steps_with(
            config,
            lane,
            names,
            picks,
            windowed,
            cached_shots,
            &program_of,
        )?;
        if steps.is_empty() {
            return Err(no_lane_steps(lane));
        }
        let shots = ShotsCache::of(
            places,
            &self.source,
            repo,
            lane,
            environment,
            sha,
            tree,
            picks,
            cached_shots,
            retake,
            base_run,
            built,
            &programs.builds(),
        )?;
        let from = timing.steps.len();
        let ended = self.run_steps_in(site, held, built_file, timing, steps, &shots, config);
        for step in &mut timing.steps[from..] {
            step.at_merge_base = Some(base_run);
        }
        ended
    }

    /// Runs <steps>, the lane's steps that build and then its test and shots
    /// runs, in <site> with <shots> and the settings of <config>, the
    /// `.app-build` of the commit the site is on; and ends as `left_unchanged`
    /// says.
    #[allow(clippy::too_many_arguments)]
    fn run_steps_in(
        &self,
        site: &mut Site,
        held: &lock::Held,
        built_file: &Path,
        timing: &mut Timing,
        steps: Vec<Step>,
        shots: &ShotsCache,
        config: &Config,
    ) -> Result<Ended> {
        let (tests, builds): (Vec<Step>, Vec<Step>) = steps
            .into_iter()
            .partition(|step| pick_kind_of(&step.name).is_some() || is_capture(&step.name));
        let with = |results| StepsWith {
            results,
            shots,
            programs: self.programs,
            watch: crate::app_platform::of(self.lane).builds_with_xcode(),
        };
        let results = config.results_of(self.lane);
        let ran = (|| {
            let mut ended = site.run_steps(held, &builds, &with(&[]), &mut timing.steps)?;
            if ended == Ended::Finished && !builds.is_empty() {
                let steps = builds.iter().map(|step| step.name.clone()).collect();
                Built {
                    commit: site.head()?,
                    steps,
                }
                .write(built_file)?;
            }
            if ended == Ended::Finished {
                ended = site.run_steps(held, &tests, &with(&results), &mut timing.steps)?;
            }
            Ok(ended)
        })();
        left_unchanged(site, built_file, ran)
    }

    /// Makes the cache hold the run of each shots pick of the lane at the
    /// merge base <base>, which the commit's shots compare with, in <site>:
    /// the picks of <base> that have no cached run. The lane checks the merge
    /// base out, runs the lane's `needs` steps there (none when the picture
    /// folder caches every shot the runs take), runs the missing prefixes
    /// with `--base`, and caches their findings, holding the lock on the merge base's runs, so another
    /// lane's process that needs the same runs waits for them and reads them
    /// from the cache. The lane then goes on with the commit in the same
    /// turn. A run of the merge base compares with nothing, so the findings
    /// it lists fail nothing; a step that fails without cached findings fails
    /// the build. Returns how the lane ended and the prefixes whose run was
    /// missing when it looked.
    fn ensure_base(
        &self,
        base: &MergeBase,
        environment: &str,
        site: &mut Site,
        held: &lock::Held,
        built_file: &Path,
        timing: &mut Timing,
    ) -> LaneResult<(Ended, Vec<(shots::Prefix, MissCause)>)> {
        let LaneBuild {
            places,
            repo,
            lane,
            programs,
            ..
        } = *self;
        let commit = self.request.of.commit;
        let program = programs.builds();
        let missing = |prefixes: &[shots::Prefix]| -> Result<Vec<shots::Prefix>> {
            let mut missing = Vec::new();
            for prefix in prefixes {
                if crate::shots_compare_again::before_run(
                    places,
                    repo,
                    &base.commit,
                    lane,
                    environment,
                    prefix.as_str(),
                    &program,
                )?
                .is_none()
                {
                    missing.push(prefix.clone());
                }
            }
            Ok(missing)
        };
        let asked = missing(&base.prefixes)?;
        if asked.is_empty() {
            return Ok((Ended::Finished, Vec::new()));
        }
        // The cause is what the cache holds before this build, or one it
        // waits for, adds a run.
        let causes = asked
            .iter()
            .map(|prefix| {
                let cause = shots_cache::why_missed(
                    places,
                    repo,
                    &base.commit,
                    lane,
                    environment,
                    prefix.as_str(),
                    &crate::shots_program::VERSIONS,
                )?;
                Ok((prefix.clone(), cause))
            })
            .collect::<Result<Vec<_>>>()?;
        let Some(_lock) = lock_base_run(places, repo, lane, &base.commit, environment)? else {
            return Ok((Ended::Stopped, Vec::new()));
        };
        let wanted = missing(&asked)?;
        if wanted.is_empty() {
            note(format!(
                "another build cached the runs at the merge base {} that these runs compare with",
                short(&base.commit)
            ));
            return Ok((Ended::Finished, causes));
        }
        let versions = &crate::shots_program::VERSIONS;
        note(format!(
            "builds has no cached run of {} on {} at {}, the merge base of {} with origin/main, made in environment {environment} by shots checker {} (comparison) over {} (capture): building the merge base in this turn and running only them there",
            prefixes_of(&wanted),
            lane.word(),
            short(&base.commit),
            short(commit),
            versions.comparison,
            versions.capture
        ));
        Built::clear(built_file)?;
        let CheckedOut { sha, config } = site.check_out(places, repo, &base.commit)?;
        let tree = picture_tree(&self.source, &sha, &config, lane, environment)?;
        let pictures = shots_cache::AppPictures::of(places, repo, lane, &tree);
        let query = capture_query(&config, lane, &wanted)?;
        let selection = MergeBase::commands(lane, &wanted);
        let ran = pictures.while_held(Some(&query), |cached_shots| {
            let run = CommitRun {
                sha: &sha,
                config: &config,
                tree,
                names: &base.names,
                picks: &selection,
                windowed: &base.windowed,
                environment,
                cached_shots,
                retake: false,
                base_run: true,
                built: &[],
            };
            self.run_commit(site, held, built_file, timing, run)
        })?;
        let failed = match ran {
            Ok(Ended::Finished) => None,
            Ok(other) => return Ok((other, Vec::new())),
            // A shots run whose pairs fail it cached its findings, and the
            // steps after it ran: what matters is whether every run is cached.
            Err(Fail::Exit(code)) => Some(format!("a step failed (exit {code})")),
            Err(other) => return Err(other.into()),
        };
        if let Some(pick) = missing(&wanted)?.first() {
            let prefix = pick.as_str();
            let why = failed.unwrap_or_else(|| "it cached no findings".to_string());
            return Err(Fail::Message(format!(
                "the run of {prefix} on {} at {}, the merge base of {} with origin/main, gave nothing to compare with: {why}",
                lane.word(),
                short(&base.commit),
                short(commit)
            )).into());
        }
        if let Some(why) = failed {
            note(format!(
                "the build of the merge base failed ({why}), and builds cached the findings of its shots runs, which the runs of {} compare with",
                short(commit)
            ));
        }
        Ok((Ended::Finished, causes))
    }
}

/// <ran>, how the steps in <site> ended, unless they left the checkout
/// changed. A step that changes a file the commit holds, or adds one that no
/// ignore rule covers, built something other than the commit, and the next
/// build would refuse the changed checkout. So the build fails naming the
/// files, after it puts the checkout back on its commit and clears the record
/// of what the checkout built. Steps that failed fail with their own error,
/// and the files changed go to stderr.
fn left_unchanged(site: &mut Site, built_file: &Path, ran: Result<Ended>) -> Result<Ended> {
    let changed = match (site.changes(), &ran) {
        (Ok(changed), _) => changed,
        (Err(error), Ok(_)) => return Err(error),
        (Err(error), Err(_)) => {
            note(error.text());
            return ran;
        }
    };
    if changed.is_empty() {
        return ran;
    }
    Built::clear(built_file)?;
    site.put_back()?;
    let message = format!(
        "the steps changed these files in {}, so they did not build the commit as it is; builds put the checkout back on its commit:\n  {}",
        site.shown()?,
        changed.join("\n  ")
    );
    match ran {
        Ok(_) => fail!("{message}"),
        Err(error) => {
            note(message);
            Err(error)
        }
    }
}

/// The requested picks left to run and the shots comparisons the parent reused.
pub struct BuildPicks {
    pub pending: Vec<Pick>,
    cached: CachedPicks,
}

enum CachedPicks {
    Reused(Vec<(Lane, shots::Prefix)>),
    Failed(NonZeroU8),
}

impl BuildPicks {
    pub fn pending(picks: Vec<Pick>) -> Self {
        Self {
            pending: picks,
            cached: CachedPicks::Reused(Vec::new()),
        }
    }

    pub fn cached_result(&self) -> Result<()> {
        match self.cached {
            CachedPicks::Failed(code) => Err(Fail::Exit(i32::from(code.get()))),
            CachedPicks::Reused(_) => Ok(()),
        }
    }

    pub fn reused_in(&self, lane: Lane) -> impl Iterator<Item = &str> {
        let reused = match &self.cached {
            CachedPicks::Reused(picks) => picks.as_slice(),
            CachedPicks::Failed(_) => &[],
        };
        reused
            .iter()
            .filter(move |(own, _)| *own == lane)
            .map(|(_, prefix)| prefix.as_str())
    }
}

/// Prints cached findings when a passing app build of <commit> covers the
/// prefix and the cached comparison has a passing app-build verdict. Keeps
/// picks that need a comparison. A repo in no project reports each cached
/// finding and keeps the first failed status.
pub fn serve_cached(
    places: &Places,
    repo: &str,
    commit: &str,
    picks: Vec<Pick>,
    program: &Path,
    recorded: Option<&BuildRecords<'_>>,
    declarations: &mut ShotDeclarations<'_>,
) -> Result<BuildPicks> {
    let mut pending = Vec::new();
    let mut reused = Vec::new();
    let mut failed = None;
    for pick in picks {
        let Pick::Shots(lane, ref prefix) = pick else {
            pending.push(pick);
            continue;
        };
        let name = prefix.as_str();
        let Some(cached) =
            crate::shots_compare_again::lookup_newest(places, repo, commit, lane, name, program)?
        else {
            pending.push(pick);
            continue;
        };
        match (recorded, NonZeroU8::new(cached.app_exit_code())) {
            (Some(_), Some(_)) => {
                note(format!(
                    "The cache has no passing comparison for {name}, so the build runs it again."
                ));
                pending.push(pick);
                continue;
            }
            (Some(records), None) => {
                if !records.has_passing_run(lane, commit, |run| {
                    covers_prefix(
                        run,
                        name,
                        (lane, commit),
                        &mut |lane, commit| declarations.names(lane, commit),
                        &std::collections::HashSet::new(),
                    )
                })? {
                    note(format!(
                        "{name} has a cached run, but no passing app build of {} ran it, so the build runs it again",
                        short(commit)
                    ));
                    pending.push(pick);
                    continue;
                }
                reused.push((lane, prefix.clone()));
            }
            (None, Some(code)) => {
                if cached.exit_code() != code.get() {
                    note(format!(
                        "The source comparison for {name} failed with exit status {code}."
                    ));
                }
                failed.get_or_insert(code);
            }
            (None, None) => {}
        }
        note("printing the cached findings instead of running (--fresh runs)");
        shot_findings::print(&cached);
        shot_findings::note_report(&cached);
    }
    Ok(BuildPicks {
        pending,
        cached: failed.map_or(CachedPicks::Reused(reused), CachedPicks::Failed),
    })
}

/// The prefixes of <picks>, joined for a line of output.
fn prefixes_of(prefixes: &[shots::Prefix]) -> String {
    prefixes
        .iter()
        .map(shots::Prefix::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The pids of the lanes' processes, for the signal handler to pass on to.
static LANE_PIDS: [AtomicI32; 2] = [AtomicI32::new(0), AtomicI32::new(0)];
/// The stop signal the command got and passed on to the lanes; 0 before.
static PASSED_ON: AtomicI32 = AtomicI32::new(0);

extern "C" fn pass_to_lanes(signal: libc::c_int) {
    PASSED_ON.store(signal, Ordering::SeqCst);
    for pid in &LANE_PIDS {
        let pid = pid.load(Ordering::SeqCst);
        if pid > 0 {
            unsafe { libc::kill(pid, signal) };
        }
    }
}

/// How many of a lane's last lines of output the build's failure message
/// repeats.
const TAIL_LINES: usize = 30;

/// What a lane said last: its last `builds:` line and its last lines of
/// output, as printed, for the message of a build that fails.
#[derive(Default)]
struct Tail {
    last_note: String,
    lines: std::collections::VecDeque<String>,
}

impl Tail {
    fn push(&mut self, printed: &str) {
        let text = printed.trim_end();
        if text.is_empty() {
            return;
        }
        if text.contains("builds: ") && !text.contains("cached its findings") {
            self.last_note = text.to_string();
        }
        if self.lines.len() == TAIL_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(text.to_string());
    }
}

/// Copies <from> to <to>, each line behind <prefix>; sends on <done> at the
/// end.
fn copy_lines(
    from: impl Read,
    mut to: impl Write,
    lane: String,
    prefix: Arc<Mutex<String>>,
    tail: Arc<Mutex<Tail>>,
    done: std::sync::mpsc::Sender<()>,
) {
    let mut from = BufReader::new(from);
    let mut line = Vec::new();
    while let Ok(read) = from.read_until(b'\n', &mut line) {
        if read == 0 {
            break;
        }
        if let Some(named) = prefix_from_marker(&lane, &line) {
            *prefix.lock().expect("no panic holding it") = named;
            line.clear();
            continue;
        }
        let mut out = prefix
            .lock()
            .expect("no panic holding it")
            .clone()
            .into_bytes();
        out.extend_from_slice(&line);
        if !line.ends_with(b"\n") {
            out.push(b'\n');
        }
        let _ = to.write_all(&out);
        tail.lock()
            .expect("no panic holding it")
            .push(&String::from_utf8_lossy(&out));
        line.clear();
    }
    let _ = done.send(());
}

/// Holds the folder of this build's programs (programs.rs) for as long as the
/// value returned lives. The build's lanes run its builds, with the folder
/// first on PATH, so an install that puts other programs on PATH meanwhile
/// changes nothing the build runs. When an install replaced this program
/// after it started and before it copied it, this process has started
/// nothing yet, and runs the installed builds in its place.
pub fn pin(places: &Places) -> Result<Pinned> {
    match crate::programs::pin_this(&places.home)? {
        crate::programs::Pin::Pinned(pinned) => Ok(pinned),
        crate::programs::Pin::Replaced(why) => Err(install_gate::run_installed(&format!(
            "{why}, so this build runs the installed builds program in its place"
        ))),
    }
}

/// What a request builds: the commit, the ref the caller named it by, and the
/// project whose store records the runs (`app_timing::recording_project`),
/// None for a repo in no project.
#[derive(Clone, Copy)]
pub struct BuildOf<'a> {
    pub commit: &'a str,
    /// A lane's run records it as the ref it built (`BUILT_REF`).
    pub named: &'a str,
    pub project: Option<&'a str>,
}

/// The arguments after `builds` that start <lane>'s process for the run of
/// <of>: the flags it is given first
/// (`--built-ref`, `--project` when a project records the run, whether its steps are only what its shots need, `--fresh`),
/// then the steps <names> and the picks of <lane> in <picks>.
fn lane_words(
    repo: &str,
    lane: Lane,
    of: BuildOf,
    (needs_only, fresh): (bool, bool),
    (names, picks): (&[String], &[Pick]),
) -> Vec<String> {
    let mut words: Vec<String> = ["app-build", repo, "--lane", lane.word(), of.commit]
        .into_iter()
        .chain([BUILT_REF, of.named])
        .chain(of.project.into_iter().flat_map(|name| [PROJECT, name]))
        .chain(needs_only.then_some(NEEDS_ONLY))
        .chain(fresh.then_some(FRESH))
        .map(str::to_string)
        .collect();
    words.extend(names.iter().cloned());
    words.extend(
        picks
            .iter()
            .filter(|pick| pick.lane() == lane)
            .flat_map(pick_words),
    );
    words
}

/// Runs the lanes in <lanes> at the same time, each as a process of its own
/// from the folder <pinned>; fails, naming the lanes, if any does. The lanes
/// of <needs_only> have only the steps their shots picks need (`NEEDS_ONLY`).
/// A stop signal goes on to each lane, and the command, once they have
/// ended, fails saying it stopped.
pub fn run_lanes(
    pinned: &Pinned,
    repo: &str,
    of: BuildOf,
    lanes: &[(Lane, Vec<String>)],
    picks: &BuildPicks,
    needs_only: &[Lane],
    fresh: bool,
) -> Result<()> {
    unsafe {
        for signal in lock::STOP_SIGNALS {
            libc::signal(signal, pass_to_lanes as *const () as libc::sighandler_t);
        }
    }
    let mut running = Vec::new();
    for (at, (lane, names)) in lanes.iter().enumerate() {
        let mut command = Command::new(pinned.builds());
        command
            .args(lane_words(
                repo,
                *lane,
                of,
                (needs_only.contains(lane), fresh),
                (names, &picks.pending),
            ))
            .env(PREFIXED, "1")
            .env(
                REUSED_SHOTS,
                serde_json::to_string(&picks.reused_in(*lane).collect::<Vec<_>>())
                    .expect("shot name strings serialize"),
            )
            .env("PATH", pinned.path(std::env::var_os("PATH").as_deref()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        install_gate::set_lane_generation(&mut command);
        let mut child = command
            .spawn()
            .context(|| format!("starting the {} lane's build", lane.word()))?;
        LANE_PIDS[at].store(child.id() as i32, Ordering::SeqCst);
        let (done, ended) = channel();
        // Both streams share it: it changes once the lane's process says which
        // checkout it builds in.
        let prefix = Arc::new(Mutex::new(format!("[{}] ", lane.word())));
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let (out_done, err_done) = (done.clone(), done);
        let (word, out_prefix) = (lane.word().to_string(), prefix.clone());
        let err_word = word.clone();
        let tail = Arc::new(Mutex::new(Tail::default()));
        let (out_tail, err_tail) = (tail.clone(), tail.clone());
        std::thread::spawn(move || {
            copy_lines(
                stdout,
                std::io::stdout(),
                word,
                out_prefix,
                out_tail,
                out_done,
            )
        });
        std::thread::spawn(move || {
            copy_lines(
                stderr,
                std::io::stderr(),
                err_word,
                prefix,
                err_tail,
                err_done,
            )
        });
        running.push((*lane, child, ended, tail));
    }
    let mut failed = Vec::new();
    let mut tails = Vec::new();
    let mut waiting: Vec<_> = running.into_iter().collect();
    // Each lane is looked at as it ends, so a lane that fails says so while
    // the other still builds, not after it.
    while !waiting.is_empty() {
        let mut at = 0;
        while at < waiting.len() {
            let status = waiting[at]
                .1
                .try_wait()
                .context(|| format!("waiting for the {} lane's build", waiting[at].0.word()))?;
            let Some(status) = status else {
                at += 1;
                continue;
            };
            let (lane, _, ended, tail) = waiting.remove(at);
            for _ in 0..2 {
                if ended.recv_timeout(OUTPUT_GRACE) == Err(RecvTimeoutError::Timeout) {
                    break;
                }
            }
            if status.success() {
                continue;
            }
            let tail = tail.lock().expect("no panic holding it");
            let why = if tail.last_note.is_empty() {
                "it printed no builds: line".to_string()
            } else {
                format!("its last builds: line was {:?}", tail.last_note)
            };
            let ended = match status.code() {
                Some(code) => format!("exit {code}"),
                None => status.to_string(),
            };
            note(format!(
                "the {} lane failed ({ended}); its last lines are repeated when the build ends",
                lane.word()
            ));
            failed.push(format!("the {} lane ({ended}; {why})", lane.word()));
            tails.push((lane, tail.lines.iter().cloned().collect::<Vec<_>>()));
        }
        if !waiting.is_empty() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        let mut message = match PASSED_ON.load(Ordering::SeqCst) {
            0 => format!("app-build failed in {}", failed.join(" and ")),
            signal => format!(
                "app-build stopped on {}, which ended {}",
                lock::signal_name(signal),
                failed.join(" and ")
            ),
        };
        for (lane, lines) in tails {
            message.push_str(&format!(
                "\nthe last {} lines of the {} lane:\n{}",
                lines.len(),
                lane.word(),
                lines.join("\n")
            ));
        }
        fail!("{message}")
    }
}

/// Returns the commit from the command line <argv> of a lane's process,
/// `builds app-build <repo> --lane <lane> <commit> ...`, which a request
/// starts for each lane (`run_lanes`) and which waits in that lane's queue.
/// Returns None for any other command.
pub fn lane_requested(argv: &[String]) -> Option<&str> {
    match crate::ack::agent::without_flag(argv)[..] {
        [_, "app-build", _repo, "--lane", _lane, commit, ..] if !commit.starts_with('-') => {
            Some(commit)
        }
        _ => None,
    }
}

/// Returns the repo and the ref (the branch or commit to build) from the
/// command line <argv> of a running `builds app-build <repo> <ref> ...`
/// request. Returns None for any other command, for `--list`, and for a
/// lane's process (`--lane`), which the request starts and waits for.
pub fn requested(argv: &[String]) -> Option<(&str, &str)> {
    match crate::ack::agent::without_flag(argv)[..] {
        [_, "app-build", repo, reference, ..]
            if !repo.starts_with('-') && !reference.starts_with('-') =>
        {
            Some((repo, reference))
        }
        _ => None,
    }
}

/// `builds app-build`. The floor of <tool>'s rules is the free-disk floor a
/// host keeps when a lane adds a checkout there (lane_size.rs).
pub fn cmd_app_build(tool: &Tool, args: &[String]) -> Result<()> {
    let places = &tool.places;
    if let [flag, rest @ ..] = args
        && flag == crate::app_landing::FLAG
    {
        return crate::app_landing::cmd_landing_coverage(places, rest);
    }
    if let [flag, rest @ ..] = args
        && flag == crate::app_shots_drop::FLAG
    {
        return crate::app_shots_drop::cmd_drop_shots(tool, rest);
    }
    if let [flag, rest @ ..] = args
        && flag == crate::handed_runs::FLAG
    {
        return crate::handed_runs::cmd_record_handed(places, rest);
    }
    let [repo, rest @ ..] = args else {
        fail!("{USAGE}");
    };
    if let [flag] = rest
        && flag == "--list"
    {
        let top = local_source(places, repo)?;
        let file = top.join(CONFIG);
        let text =
            std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
        let config = parse_config(&text)?;
        for step in &config.steps {
            let ci_only = if step.ci_only { " (CI only)" } else { "" };
            println!(
                "{} {} {}{ci_only}",
                step.lane.word(),
                step.name,
                step.command.join(" ")
            );
        }
        for picker in &config.pickers {
            let named =
                crate::app_config::lane_and_program_word(picker.lane, picker.program.as_deref());
            let place = if picker.opens_windows {
                " (opens windows)"
            } else {
                ""
            };
            println!(
                "{named} {} <{}> {}{place}",
                picker.kind.flag(),
                picker.kind.what(),
                picker.command.join(" ")
            );
        }
        return Ok(());
    }
    if let [flag, word, commit, names @ ..] = rest
        && flag == "--lane"
        && !commit.starts_with('-')
    {
        let [built_ref_flag, named, names @ ..] = names else {
            fail!("{USAGE}");
        };
        if built_ref_flag != BUILT_REF {
            fail!("{USAGE}");
        }
        let (flag, names) = match names.split_first() {
            Some((first, [project, rest @ ..])) if first == PROJECT => {
                (Some(project.as_str()), rest)
            }
            _ => (None, names),
        };
        // The request that started the lane chose the project, and a claim
        // that changed while the lane queued does not change it. A lane run
        // by hand chooses it as a request does.
        let project = match flag {
            Some(_) => crate::lists::store::project_named(places, repo, flag)?,
            None => crate::app_timing::recording_project(places, repo, named, None)?,
        };
        let (needs_only, names) = strip_leading(names, NEEDS_ONLY);
        let (fresh, names) = strip_leading(names, FRESH);
        let lanes = config_at(places, repo, commit)?.lanes;
        let (names, mut picks) = split_picks(names, &lanes)?;
        let lane = Lane::parse_in_or_fail(&lanes, word)?;
        if !picks.is_empty() {
            picks.retain(|pick| pick.lane() == lane);
            if names.is_empty() && picks.is_empty() {
                return Err(no_lane_steps(lane));
            }
        }
        let reused_shots: Vec<String> = match std::env::var(REUSED_SHOTS) {
            Ok(value) => serde_json::from_str(&value)
                .context(|| format!("reading the lane's reused shots from {REUSED_SHOTS}"))?,
            Err(std::env::VarError::NotPresent) => Vec::new(),
            Err(error) => fail!("reading {REUSED_SHOTS}: {error}"),
        };
        let reused_shots = reused_shots
            .iter()
            .map(|prefix| {
                shots::Prefix::parse(prefix).context(|| "reading a reused shots prefix".to_string())
            })
            .collect::<Result<Vec<_>>>()?;
        // A lane that its build started runs from the build's folder, which
        // holds its programs already; a lane run by hand copies them there.
        // The lane copies the folder to each host it runs a step on.
        let pinned = pin(places)?;
        // A lane's process always says why it ended badly: a bare exit status
        // from a path that noted nothing would reach the parent as no output.
        let request = LaneRequest {
            of: BuildOf {
                commit,
                named,
                project: project.as_deref(),
            },
            names: &names,
            picks: &picks,
            reused_shots: &reused_shots,
            needs_only,
            fresh,
        };
        return match run_lane(tool, repo, lane, request, &pinned) {
            Err(Fail::Exit(code)) => {
                note(format!(
                    "the {} lane's build failed (exit {code})",
                    lane.word()
                ));
                Err(Fail::Exit(code))
            }
            other => other,
        };
    }
    let [commit, tail @ ..] = rest else {
        fail!("{USAGE}");
    };
    if commit.starts_with('-') {
        fail!("{USAGE}");
    }
    let (flag, tail) = take_project(tail, USAGE)?;
    // Before anything builds, so that a build with nowhere to record fails
    // at once.
    let project = crate::app_timing::recording_project(places, repo, commit, flag)?;
    if tail.first().is_none_or(|arg| arg.starts_with("--")) {
        // No step named: the diff chooses them (derive.rs).
        let args: Vec<String> = [repo, commit].into_iter().cloned().chain(tail).collect();
        return crate::derive::derive_and_run(places, &args, project.as_deref());
    }
    let fresh = tail.iter().any(|arg| arg == "--fresh");
    let tail: Vec<String> = tail
        .iter()
        .filter(|arg| *arg != "--fresh")
        .cloned()
        .collect();
    let (names, picks) = split_picks(&tail, &config_at(places, repo, commit)?.lanes)?;
    let plan = explicit_plan(places, repo, commit, &names, picks)?;
    let pinned = pin(places)?;
    run_lanes(
        &pinned,
        repo,
        BuildOf {
            commit: &plan.commit,
            named: commit,
            project: project.as_deref(),
        },
        &plan.lanes,
        &BuildPicks::pending(plan.picks),
        &[],
        fresh,
    )
}

/// The project that <args>' `--project` names, and <args> without it. A
/// refusal ends with the command's <usage>.
pub fn take_project<'a>(args: &'a [String], usage: &str) -> Result<(Option<&'a str>, Vec<String>)> {
    let mut rest = args.to_vec();
    let Some(at) = args.iter().position(|arg| arg == PROJECT) else {
        return Ok((None, rest));
    };
    let value = args.get(at + 1).filter(|value| !value.starts_with('-'));
    let Some(value) = value else {
        fail!("{PROJECT} needs a project's name\n{usage}");
    };
    rest.drain(at..at + 2);
    if rest.iter().any(|arg| arg == PROJECT) {
        fail!("{PROJECT} is given twice\n{usage}");
    }
    Ok((Some(value), rest))
}

/// What an app build with named steps or picks builds: the one commit its ref
/// named when the plan was made, and the lanes with the steps to run there.
struct ExplicitPlan {
    commit: String,
    lanes: Vec<(Lane, Vec<String>)>,
    picks: Vec<Pick>,
}

/// Resolves <git_ref> once, checks <names> and <picks> against `.app-build`
/// and the declared shots at that commit, and returns the plan to build that
/// commit, so a branch that moves meanwhile changes nothing the lanes build.
fn explicit_plan(
    places: &Places,
    repo: &str,
    git_ref: &str,
    names: &[String],
    picks: Vec<Pick>,
) -> Result<ExplicitPlan> {
    let (top, commit, config) = commit_and_config(places, repo, git_ref)?;
    for name in names {
        step_named(&config, name)?;
    }
    require_pickers(&config, &picks)?;
    crate::shots_declared::refuse_undeclared(&top, &commit, &picks)?;
    // The lanes the steps or picks are in: those with a step named, or with
    // any step when none is named and no pick is, each with its own names.
    let lanes: Vec<(Lane, Vec<String>)> = config
        .lanes
        .iter()
        .copied()
        .map(|lane| (lane, names_in(&config, lane, names)))
        .filter(|(lane, own)| {
            let picks_here = picks.iter().any(|pick| pick.lane() == *lane);
            if names.is_empty() && picks.is_empty() {
                config.steps.iter().any(|step| step.lane == *lane)
            } else {
                !own.is_empty() || picks_here
            }
        })
        .collect();
    if lanes.is_empty() {
        fail!("{CONFIG} names no step");
    }
    Ok(ExplicitPlan {
        commit,
        lanes,
        picks,
    })
}

/// Whether <names> start with the flag <flag>, and <names> without it.
fn strip_leading<'a>(names: &'a [String], flag: &str) -> (bool, &'a [String]) {
    match names.split_first() {
        Some((first, rest)) if first == flag => (true, rest),
        _ => (false, names),
    }
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
