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
//! prefix in it, each run with the kind's limit: one run of each test line
//! for all of its classes, and one run of the shots line per prefix. With
//! such flags and no step named in the `--lane` form, only those runs happen.
//! An app build runs only what a change touches, so every form refuses a
//! `--shots` prefix that selects every shot of its lane (`mac.`) and a step
//! that a `ci-only` line names (app_config.rs); CI runs the full suites.
//! `--all-shots <lane>` is how a caller asks for a lane's full set of shots,
//! as CI's screenshot workflow takes them: the run of the lane's `shots` line
//! with the prefix every shot of the lane starts with, kept like any other
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
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::app_build_step_failure::StepRead;
use crate::app_config::{
    CONFIG, CheckedOut, Config, PickKind, Picker, Step, check_out, parse_config, resolve,
    run_command_keeping, run_git, show_duration,
};
use crate::app_remote::{RemoteApp, fetched_local_source, local_source};
use crate::app_site::Site;
use crate::app_timing::{self, RunRecord, StepKind, StepRecord, StepResult, Timer};
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
use crate::shot_store;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds app-build <repo> <ref> [--test <Class>]... [--shots <prefix>]... [--all-shots mac|android]... [--fresh] | builds app-build <repo> <ref> <step>... [--test mac|android <Class>]... [--shots mac|android <prefix>]... [--all-shots mac|android]... [--fresh] | builds app-build <repo> --list | builds app-build <repo> --lane mac|android <ref> [<step>...] [--test mac|android <Class>]... [--shots mac|android <prefix>]... [--all-shots mac|android]...";

/// Set by the parent in each lane's process: print the marker line below once
/// the checkout is known.
const PREFIXED: &str = "BUILDS_APP_BUILD_PREFIXED";
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
            "step {name} is CI-only in {CONFIG}, and builds refuses it: {ONLY_WHAT_A_CHANGE_TOUCHES}. Name the change's test classes with --test and its screens' shots with --shots, or run a lane's full set of shots with {ALL_SHOTS} mac|android"
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
pub type Pick = (Lane, PickKind, String);

/// The rule that app-build names when it refuses a `--test` argument that is
/// no type name, a `--shots` prefix that selects every shot of its lane, or a
/// step that a `ci-only` line names.
pub const ONLY_WHAT_A_CHANGE_TOUCHES: &str = "an app build runs only the test classes and shots of the screens a change touches, and CI runs the full suites";

/// Refuses a shots <prefix> in <lane> that selects every shot of the lane:
/// one that `<lane>.` starts with (`mac.`, `mac`), since every shot name of
/// the lane starts with `<lane>.`. `--all-shots <lane>` asks for that run.
pub fn refuse_whole_lane(lane: Lane, prefix: &str) -> Result<()> {
    let word = lane.word();
    if lane.all_shots_prefix().starts_with(prefix) {
        fail!(
            "the shots prefix {prefix} selects every shot of the {word} lane, and builds refuses it in --shots: {ONLY_WHAT_A_CHANGE_TOUCHES}. Give a prefix that names the screens the change touches, such as {word}.welcome. for the welcome screens. To check a change to the shots program or a fix for CI's shot jobs, {ALL_SHOTS} {word} runs every shot of the lane, as CI's screenshot workflow does"
        );
    }
    Ok(())
}

/// The flag that asks for a lane's full set of shots: `--all-shots mac`.
pub const ALL_SHOTS: &str = "--all-shots";

/// The pick of <lane>'s full set of shots: the run of the lane's `shots` line
/// with the prefix every shot of the lane starts with.
pub fn all_shots(lane: Lane) -> Pick {
    (lane, PickKind::Shots, lane.all_shots_prefix())
}

/// The pick of the `--all-shots <lane>` flag whose words after the flag are
/// <after>, and the words after the lane; fails naming <usage> when no lane
/// follows.
pub fn all_shots_flag<'a>(after: &'a [String], usage: &str) -> Result<(Pick, &'a [String])> {
    let [lane, rest @ ..] = after else {
        fail!("{ALL_SHOTS} needs a lane\n{usage}");
    };
    Ok((all_shots(Lane::parse_or_fail(lane)?), rest))
}

/// The command-line words that ask for <pick> in a lane's process, or in a
/// request that names steps: `--all-shots <lane>` for a lane's full set of
/// shots, else the kind's flag, the lane and the argument.
pub fn pick_words(pick: &Pick) -> Vec<&str> {
    let (lane, kind, arg) = pick;
    if *pick == all_shots(*lane) {
        vec![ALL_SHOTS, lane.word()]
    } else {
        vec![kind.flag(), lane.word(), arg.as_str()]
    }
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
fn split_picks(args: &[String]) -> Result<(Vec<String>, Vec<Pick>)> {
    let is_flag =
        |arg: &String| arg == ALL_SHOTS || PickKind::ALL.iter().any(|kind| arg == kind.flag());
    let at = args.iter().position(is_flag).unwrap_or(args.len());
    let (names, mut flags) = (args[..at].to_vec(), &args[at..]);
    let mut picks = Vec::new();
    while let Some((flag, rest)) = flags.split_first() {
        if flag == ALL_SHOTS {
            let (pick, after) = all_shots_flag(rest, USAGE)?;
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
        let lane = Lane::parse_or_fail(lane)?;
        if kind == PickKind::Shots {
            refuse_whole_lane(lane, arg)?;
        }
        picks.push((lane, kind, arg.clone()));
        flags = after;
    }
    Ok((names, picks))
}

/// The refusal for a pick of a kind whose line <config> lacks in <lane>.
pub fn missing_line(lane: Lane, kind: PickKind, arg: &str) -> Fail {
    Fail::Message(format!(
        "{CONFIG} has no `{} {} ...` line, so it cannot run the {} {arg}",
        kind.line(),
        lane.word(),
        kind.what()
    ))
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
/// builds keeps the findings of each prefix's run.
fn lane_steps(
    config: &Config,
    lane: Lane,
    names: &[String],
    picks: &[Pick],
    program_of: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<Vec<Step>> {
    let mut steps = if names.is_empty() && !picks.is_empty() {
        Vec::new()
    } else {
        chosen(config, lane, names)?
    };
    let mut runs: Vec<PickRun> = Vec::new();
    for (_, kind, arg) in picks.iter().filter(|(own, _, _)| *own == lane) {
        let program = match kind {
            PickKind::Class => program_of(arg)?,
            PickKind::Shots => None,
        };
        let Some(picker) = config.picker_in(lane, *kind, program.as_deref()) else {
            return Err(missing_line(lane, *kind, arg));
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
    for run in runs {
        let kind = run.picker.kind;
        steps.push(Step {
            name: format!("{}{}", kind.step_prefix(), run.args.join(" ")),
            lane,
            command: run.picker.command_for(&run.args),
            limit: Some(kind.limit()),
            place: run.picker.place,
            ci_only: false,
        });
    }
    Ok(steps)
}

/// Where a lane's checkout must be to run all of <steps>: where each of them
/// may run. Fails, naming two of them, when no checkout can run both.
pub fn place_of(steps: &[Step]) -> Result<lock::Where> {
    let mut narrowed: Option<&Step> = None;
    for step in steps
        .iter()
        .filter(|step| step.place != lock::Where::Anywhere)
    {
        match narrowed {
            None => narrowed = Some(step),
            Some(first) if first.place == step.place => {}
            Some(first) => fail!(
                "{} runs only {} and {} only {}, so no app checkout can run both: run them in separate app-builds",
                first.name,
                first.place.only(),
                step.name,
                step.place.only()
            ),
        }
    }
    Ok(narrowed.map_or(lock::Where::Anywhere, |step| step.place))
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
/// findings the shots store keeps (<all>), the first line saying the host
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

/// Runs <step> (of <kind>) in the checkout <top>, watching for a line of
/// `HOST_FAULTS` in <fault> and giving each line to <read>; a shots run
/// compares with its before run, read where the store <shots> keeps it,
/// takes the board pictures earlier runs drew from the store's picture
/// folder, and keeps the findings it printed. While the shots program runs,
/// the step holds the shared lock that keeps the prune from deleting what it
/// reads or writes in the folder `shots` (shot_store.rs).
fn run_local_step(
    top: &Path,
    step: &Step,
    kind: StepKind,
    shots: &ShotsStore,
    fault: Option<&Found>,
    read: &lock::EachLine,
) -> Result<Within> {
    let kept = lock::Captured::default();
    let keep = keep_for(kind, &kept, fault, read);
    let (command, _writing) = match kind {
        StepKind::Shots => (shots.local_command(step), Some(shots.writing()?)),
        _ => (step.command.clone(), None),
    };
    let outcome = run_command_keeping(top, &command, Some(step.limit()), keep);
    if kind == StepKind::Shots {
        shots.keep_of(&outcome, &kept, step, &mut LocalRun(top));
    }
    outcome
}

/// Runs <steps> in the checkout <top>, as `run_steps_with` does with <watch>.
fn run_steps(
    top: &Path,
    held: &lock::Held,
    steps: &[Step],
    log: &mut Vec<StepRecord>,
    shots: &ShotsStore,
    watch: bool,
) -> Result<Ended> {
    run_steps_with(held, steps, log, watch, &mut |step, kind, fault, read| {
        run_local_step(top, step, kind, shots, fault, read)
    })
}

/// Runs one step of a kind, watching for a line of `HOST_FAULTS` when given
/// where to keep it, and giving each line of its output to the reader.
type RunStep<'a> =
    dyn FnMut(&Step, StepKind, Option<&Found>, &lock::EachLine) -> Result<Within> + 'a;

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
    run: &mut RunStep,
) -> Result<Ended> {
    // The status of the first shots run whose pairs failed it.
    let mut pairs_failed = None;
    for step in steps {
        // A step that opens windows holds its host's display alone.
        let _window = if step.place == lock::Where::Hosts {
            match held.take_window(&step.name)? {
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
        let timer = Timer::start();
        let found = Found::default();
        let read = StepRead::default();
        let outcome = run(step, kind, watch.then_some(&found), &read.each_line());
        let failed_pairs = kind == StepKind::Shots && !read.failure().failed.is_empty();
        let stopped_by = lock::stop_signal();
        let result = match &outcome {
            Ok(Within::Ran(Ran::Finished)) => StepResult::Passed,
            _ if stopped_by.is_some() => StepResult::Stopped,
            Ok(Within::Ran(Ran::Stopped)) => StepResult::TakenOver,
            Ok(Within::TimedOut) => StepResult::TimedOut(read.failure()),
            Err(_) => StepResult::Failed(read.failure()),
        };
        log.push(timer.finish(&step.name, kind, result));
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
                    show_duration(step.limit()),
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
                    "{} kept its findings, so the lane runs its other steps",
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
    parse_config(&run_git(&source, &["show", &format!("{commit}:{CONFIG}")])?)
}

/// What a lane's build does in its app checkout (app_site.rs).
impl Site {
    /// The commit <commit> names, and the commit the checkout is on now.
    fn resolved_and_head(
        &mut self,
        places: &Places,
        repo: &str,
        commit: &str,
    ) -> Result<(String, String)> {
        let source = self.fetched_source(places, repo)?;
        Ok((resolve(&source, commit)?, self.head()?))
    }

    /// The name of the host the checkout is on.
    fn host_name(&self) -> &str {
        match self {
            Site::Local(_) => crate::host::LOCAL,
            Site::Remote(remote) => &remote.host.name,
        }
    }

    /// The cores and load of the checkout's host now; None when the host does
    /// not answer, and the function says so on stderr.
    fn load(&self, places: &Places, ssh: &Ssh) -> Option<crate::host::Load> {
        match self {
            Site::Local(_) => Some(crate::host::Load::here()),
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
            Site::Local(top) => check_out(top, commit),
            Site::Remote(remote) => {
                let source = fetched_local_source(places, repo)?;
                let sha = resolve(&source, commit)?;
                remote.switch_to(&source, &sha)?;
                let config =
                    parse_config(&run_git(&source, &["show", &format!("{sha}:{CONFIG}")])?)?;
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
            Site::Local(top) => run_steps(top, held, steps, log, with.shots, with.watch),
            Site::Remote(remote) => run_steps_with(
                held,
                steps,
                log,
                with.watch,
                &mut |step, kind, fault, read| {
                    run_remote_step(remote, step, kind, with, fault, read)
                },
            ),
        }
    }
}

/// What a lane's steps run with.
struct StepsWith<'a> {
    /// The folders a step on a host copies back here after it runs.
    results: &'a [String],
    /// Where a shots run keeps its findings here.
    shots: &'a ShotsStore,
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
    kind: StepKind,
    with: &StepsWith,
    fault: Option<&Found>,
    read: &lock::EachLine,
) -> Result<Within> {
    let StepsWith { results, shots, .. } = *with;
    let store = (kind == StepKind::Shots).then_some(shots);
    // Held while the copies read the before run and the picture folder,
    // which the prune then leaves.
    let reading = store.map(|store| store.writing()).transpose()?;
    let placed = match store.and_then(|store| store.before_of(step)) {
        Some(findings) => Some(remote.put_temporary(findings)?),
        None => None,
    };
    let pictures = match store {
        Some(store) => Some(remote.put_folder(&store.pictures(), &store.picture_parts())?),
        None => None,
    };
    drop(reading);
    let command = match (store, &pictures) {
        (Some(store), Some(there)) => store.command(step, placed.as_deref(), there),
        _ => step.command.clone(),
    };
    let held = remote.with_programs(with.programs, &command)?;
    let start = remote.step_command(&held)?;
    let kept = lock::Captured::default();
    let keep = keep_for(kind, &kept, fault, read);
    let ran = lock::run_in_group_keeping(start, &command[0], Some(step.limit()), keep);
    if let Some(there) = &placed
        && let Err(error) = remote.remove_temporary(there)
    {
        note(error.text());
    }
    if let (Some(store), Some(there)) = (store, &pictures) {
        store.bring_back_pictures(remote, there);
        store.keep_of(&ran, &kept, step, remote);
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
    let config = parse_config(&run_git(&top, &["show", &format!("{commit}:{CONFIG}")])?)?;
    Ok((top, commit, config))
}

/// The prefix of the shots run <step>, which `lane_steps` names `shots
/// <prefix>`.
fn shots_prefix(step: &Step) -> &str {
    step.name
        .strip_prefix(PickKind::Shots.step_prefix())
        .unwrap_or(&step.name)
}

/// What a lane's process is given first when the picture folder keeps every
/// shot its shots picks take for its app tree, so it builds nothing
/// (`kept_shots_lanes`).
const KEPT_SHOTS: &str = "--kept-shots";

/// The lanes of <lanes> that build nothing for <picks>: each lane whose picks
/// are all shots, which no path <changed> names in an `on` line of the lane,
/// and whose every shots prefix the picture folder of <repo> keeps for the
/// lane's app tree at <commit> (shot_store.rs). Such a lane's steps would
/// only build the app for shots the folder already keeps.
#[allow(clippy::too_many_arguments)]
pub fn kept_shots_lanes(
    places: &Places,
    repo: &str,
    top: &Path,
    commit: &str,
    config: &Config,
    changed: &[String],
    lanes: &[(Lane, Vec<String>)],
    picks: &[Pick],
) -> Result<Vec<Lane>> {
    let built_for_changes: Vec<Lane> = crate::derive::plan(config, changed, &[])
        .into_iter()
        .map(|(lane, _)| lane)
        .collect();
    let mut kept = Vec::new();
    for (lane, _) in lanes {
        let own: Vec<&Pick> = picks.iter().filter(|(own, _, _)| own == lane).collect();
        if own.is_empty()
            || own.iter().any(|(_, kind, _)| *kind != PickKind::Shots)
            || built_for_changes.contains(lane)
        {
            continue;
        }
        let Some(picker) = config.picker(*lane, PickKind::Shots) else {
            return Err(missing_line(*lane, PickKind::Shots, &own[0].2));
        };
        let windows = picker.command.iter().any(|word| word == "--windows");
        let tree = crate::app_tree::of(top, commit, config, *lane)?;
        let mut every = true;
        for (_, _, prefix) in &own {
            every &= shot_store::shots_kept(places, repo, *lane, &tree, prefix, windows)?;
        }
        if every {
            kept.push(*lane);
        }
    }
    Ok(kept)
}

/// <command>, a run of a `shots` line, comparing with the findings.json at
/// <file>: `builds shots run --before`.
fn with_before(command: &[String], file: &str) -> Vec<String> {
    let mut command = command.to_vec();
    command.extend(["--before".to_string(), file.to_string()]);
    command
}

/// Where a lane's run keeps its `--shots` runs (shot_store.rs), and the kept
/// runs they compare with.
struct ShotsStore {
    places: Places,
    repo: String,
    lane: Lane,
    /// The version of the shots checker the store keys the runs on: the one
    /// this builds is built with.
    version: String,
    /// The findings.json of the run each shots prefix of the lane compares
    /// with, by its prefix.
    befores: Vec<(String, std::path::PathBuf)>,
    /// The lane's app source tree at the commit (app_tree.rs), which keys
    /// the shots the app takes in the picture folder.
    app_tree: String,
    /// The lane built no app, so a shots run fails instead of capturing
    /// (`builds shots run --no-capture`).
    no_capture: bool,
}

impl ShotsStore {
    /// The store for <repo>'s <lane> run of the full commit <commit>, whose
    /// commits <source> holds, with the before run of each shots pick of
    /// <picks> in the lane: the run of <commit>'s merge base with origin/main
    /// (shot_store.rs). A commit on origin/main is its own merge base and has
    /// none. Fails when the store holds no before run for a pick, naming the
    /// command that makes one: `builds app-build` with no lane named makes
    /// them first.
    #[allow(clippy::too_many_arguments)]
    fn of(
        places: &Places,
        source: &Path,
        repo: &str,
        lane: Lane,
        commit: &str,
        config: &Config,
        picks: &[Pick],
        kept_shots: bool,
    ) -> Result<ShotsStore> {
        let version = crate::shots_program::VERSION.to_string();
        let mut befores = Vec::new();
        for (_, _, prefix) in picks
            .iter()
            .filter(|(own, kind, _)| *own == lane && *kind == PickKind::Shots)
        {
            let base = crate::derive::merge_base(source, commit)?;
            if base == commit {
                continue;
            }
            match shot_store::before_run(places, repo, &base, lane, prefix, &version)? {
                Some(before) => befores.push((prefix.clone(), before.findings_json())),
                None => fail!(
                    "builds holds no run of {prefix} on {} for {}, the merge base of {} with origin/main, by shots checker {version}, so the run has nothing to compare with; `{}` makes one",
                    lane.word(),
                    &base[..base.len().min(12)],
                    &commit[..commit.len().min(12)],
                    shot_findings::how_to_make(repo, &base, lane, prefix)
                ),
            }
        }
        Ok(ShotsStore {
            places: places.clone(),
            repo: repo.to_string(),
            lane,
            version,
            befores,
            app_tree: crate::app_tree::of(source, commit, config, lane)?,
            no_capture: kept_shots,
        })
    }

    /// The shared lock a shots step holds while it reads a before run or the
    /// picture folder, or writes to the folder (shot_store.rs `writing`).
    fn writing(&self) -> Result<std::fs::File> {
        shot_store::writing(&self.places)
    }

    /// The command of the shots run <step> in a checkout on this Mac: its
    /// line, comparing with its before run when its prefix has one, and
    /// taking the board pictures earlier runs drew and the shots earlier
    /// runs of the lane's app tree took from the picture folder
    /// (`builds shots run --pictures --app-tree`).
    fn local_command(&self, step: &Step) -> Vec<String> {
        let before = self
            .before_of(step)
            .map(|findings| findings.display().to_string());
        let pictures = self.pictures().display().to_string();
        self.command(step, before.as_deref(), &pictures)
    }

    /// The command of the shots run <step>: its line, comparing with the
    /// findings.json at <before> when there is one, with the picture folder
    /// at <pictures> and the lane's app tree.
    fn command(&self, step: &Step, before: Option<&str>, pictures: &str) -> Vec<String> {
        let mut command = match before {
            Some(findings) => with_before(&step.command, findings),
            None => step.command.clone(),
        };
        command.extend([
            "--pictures".to_string(),
            pictures.to_string(),
            "--app-tree".to_string(),
            self.app_tree.clone(),
        ]);
        if self.no_capture {
            command.push("--no-capture".to_string());
        }
        command
    }

    /// The repo's picture folder on this Mac.
    fn pictures(&self) -> std::path::PathBuf {
        shot_store::pictures(&self.places, &self.repo)
    }

    /// The parts of the picture folder a run on a host reads: the board
    /// pictures and the shots kept for the lane's app tree.
    fn picture_parts(&self) -> Vec<String> {
        vec![
            shot_store::BOARD_PICTURES.to_string(),
            format!("{}/{}", self.lane.word(), self.app_tree),
        ]
    }

    /// The findings.json of the run the shots run <step> compares with, when
    /// its prefix has one.
    fn before_of(&self, step: &Step) -> Option<&Path> {
        let prefix = shots_prefix(step);
        self.befores
            .iter()
            .find(|(own, _)| own == prefix)
            .map(|(_, findings)| findings.as_path())
    }

    /// Keeps what the shots run <step> printed (<kept>), when it ended
    /// (<outcome>) with an exit status, as the run of the commit the site is
    /// on.
    fn keep_of(
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
        self.keep(site, shots_prefix(step), exit, &text);
    }

    /// Keeps a run of <prefix> that exited <exit> and printed <output> as the
    /// run of the commit the site is on: what the shots program printed and
    /// the report it names, copied from the site (shot_findings.rs). A run
    /// whose report's findings.json names another version of the shots
    /// checker than the store's is not kept, since no lookup of this builds would
    /// find it under its own version. A run that printed no findings, or a
    /// write or copy that fails, is said and changes nothing about the
    /// build's result; a report that fails to copy leaves the run kept
    /// without one.
    fn keep(&self, site: &mut dyn RunSite, prefix: &str, exit: i64, output: &str) {
        let findings = shot_findings::findings_of(output);
        if !shot_findings::has_findings(exit, findings) {
            note(format!(
                "the shots run of {prefix} exited {exit} without findings, so none were kept"
            ));
            return;
        }
        let staged = site.commit().and_then(|commit| {
            let key = shot_store::Key {
                repo: self.repo.clone(),
                version: self.version.clone(),
                commit,
                lane: self.lane,
                prefix: prefix.to_string(),
            };
            shot_store::stage(&self.places, &key)
        });
        let staging = match staged {
            Ok(staging) => staging,
            Err(error) => {
                note(format!("the shots run was not kept: {}", error.text()));
                return;
            }
        };
        let report = staging.report();
        match shot_findings::report_dir_in(output) {
            Some(dir) => {
                if let Err(error) = site.fetch_report(&dir, &report) {
                    note(format!("the shots report was not kept: {}", error.text()));
                    if let Err(error) = staging.discard_report() {
                        note(error.text());
                        return;
                    }
                } else if let Err(why) = self.same_version(&report) {
                    note(format!("the shots run was not kept: {why}"));
                    if let Err(error) = staging.discard() {
                        note(error.text());
                    }
                    return;
                }
            }
            None => note(format!(
                "the shots run of {prefix} printed no report path, so no report was kept"
            )),
        }
        match staging.finish(exit, findings) {
            Ok(stored) => shot_findings::note_report(&stored),
            Err(error) => note(format!("the shots run was not kept: {}", error.text())),
        }
    }

    /// Copies the pictures a run on the host drew or took into the picture
    /// folder <there> on the host into the repo's picture folder here, and
    /// deletes <there>. What fails is said and changes nothing about the
    /// build's result.
    fn bring_back_pictures(&self, remote: &mut RemoteApp, there: &str) {
        let pictures = self.pictures();
        let incoming = shot_store::incoming(&pictures);
        // The lock keeps the prune from deleting the incoming folder while
        // the copy fills it.
        let brought = self.writing().and_then(|_writing| {
            remote.fetch_folder(there, &incoming)?;
            shot_store::merge_pictures(&incoming, &pictures)
        });
        if let Err(error) = brought {
            note(format!(
                "the pictures of the run on host {} were not kept: {}",
                remote.host.name,
                error.text()
            ));
        }
        if let Err(error) = remote.remove_temporary(there) {
            note(error.text());
        }
    }

    /// Whether the findings.json in the report folder <folder> names the
    /// store's version of the shots checker; why not, when it names another.
    fn same_version(&self, folder: &Path) -> std::result::Result<(), String> {
        let named = shot_findings::version_in(folder).map_err(|error| error.text())?;
        if named == self.version {
            return Ok(());
        }
        Err(format!(
            "the run's findings.json names shots checker {named}, and this builds keeps the runs of shots checker {}: the run's `builds shots` was another builds than this one",
            self.version
        ))
    }
}

/// Where a shots run ran: what `ShotsStore::keep` asks of it.
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

impl RunSite for RemoteApp {
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

/// When a run joined the lane's queue, how long it waited there in all, and
/// the checkout whose lock it holds, as `lock::Held` keeps them, with the
/// load of the checkout's host when the run took the checkout.
struct HeldTimes {
    joined_at: i64,
    wait: Duration,
    checkout: String,
    load: Option<crate::host::Load>,
}

impl HeldTimes {
    fn of(held: &lock::Held, load: Option<crate::host::Load>) -> HeldTimes {
        HeldTimes {
            joined_at: held.joined_at,
            wait: held.waited,
            checkout: held.checkout.clone(),
            load,
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

    /// Writes the run, which ended with <outcome>, to the repo's project
    /// store when it took a checkout. When an install moved the store to
    /// another schema while the run built, the run goes to the store through
    /// the installed builds (handed_runs.rs). A repo in no project, or a
    /// write that fails, is said and changes nothing about the build's
    /// result.
    fn record(self, places: &Places, repo: &str, lane: Lane, built_ref: &str, outcome: RunEnd) {
        let Some(held) = self.held else {
            return;
        };
        let run = RunRecord {
            started_at: held.joined_at,
            repo: repo.to_string(),
            lane,
            checkout: held.checkout,
            built_ref: built_ref.to_string(),
            built_commit: self.commit,
            who: who_asked(),
            wait_ms: held.wait.as_millis() as u64,
            outcome,
            load: held.load,
            steps: self.steps,
        };
        let installed = crate::links::path(&places.home, "builds");
        let written = app_timing::database_of(places, repo).and_then(|database| {
            let Some(database) = database else {
                return Ok(());
            };
            crate::handed_runs::write(
                &database,
                &run,
                &crate::handed_runs::folder(places),
                &install_gate::install_ran,
                &mut |store| crate::handed_runs::hand_to(&installed, repo, store),
            )
        });
        if let Err(error) = written {
            note(format!("this run was not recorded: {}", error.text()));
        }
    }
}

/// The claim of the worktree this process runs in: who asked for the build.
fn who_asked() -> Option<String> {
    let checkout = Checkout::at(Path::new(".")).ok()??;
    let claim = Claim::read(&checkout.claim_file()).ok()??;
    Some(claim.who).filter(|who| !who.is_empty())
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
    timing.record(&tool.places, repo, lane, request.commit, outcome);
    result
}

/// What a caller asked one lane to build: <commit>, with the lane's steps
/// <names> (all of them when none and no <picks>), then the test classes and
/// shots of <picks>.
#[derive(Clone, Copy)]
struct LaneRequest<'a> {
    commit: &'a str,
    names: &'a [String],
    picks: &'a [Pick],
    /// The picture folder keeps every shot of the shots picks for the
    /// lane's app tree (`KEPT_SHOTS`): the lane builds nothing, and its shots
    /// runs capture none.
    kept_shots: bool,
}

fn build_lane(
    tool: &Tool,
    repo: &str,
    lane: Lane,
    request: LaneRequest,
    programs: &Pinned,
    timing: &mut Timing,
) -> Result<()> {
    let places = &tool.places;
    let floor_kib = tool.rules.floor_kib;
    let LaneRequest {
        commit,
        names,
        picks,
        kept_shots,
    } = request;
    note(format!(
        "waiting for {repo}'s {} app lock, then building {commit}",
        lane.word()
    ));
    let ssh = Ssh::from_env();
    // A step the repo keeps on this Mac (`here` lines) keeps the build off
    // the hosts' checkouts, and a test run that opens windows (`opens-windows`
    // lines) keeps it off this Mac's.
    let config = config_at(places, repo, commit)?;
    let source = local_source(places, repo)?;
    let program_of = |class: &str| crate::derive::program_of(&source, &config, commit, lane, class);
    let steps = lane_steps(&config, lane, names, picks, &program_of)?;
    let allowed = place_of(&steps)?;
    if let Some(windowed) = steps.iter().find(|step| step.place == lock::Where::Hosts) {
        note(format!(
            "{} opens windows, so it runs only in an app checkout on a host, never on this Mac, where the windows would cover the person's screen",
            windowed.name
        ));
    }
    if allowed == lock::Where::Here {
        crate::worktree::ensure_local_app_checkout(tool, &ssh, repo, lane)?;
    }
    // A shots run of a line that opens a window for each shot draws on the
    // host's screen.
    let draws = picks
        .iter()
        .any(|(own, kind, _)| *own == lane && *kind == PickKind::Shots)
        && config
            .picker(lane, PickKind::Shots)
            .is_some_and(|picker| picker.place == lock::Where::Hosts);
    let mut held = if draws {
        lock::take_drawing(places, &ssh, repo, lane, floor_kib)?
    } else {
        lock::take_where(places, &ssh, repo, lane, allowed, floor_kib)?
    };
    loop {
        let mut site = Site::of(places, &ssh, repo, lane, &held.checkout)?;
        timing.held = Some(HeldTimes::of(&held, site.load(places, &ssh)));
        note(format!("building in {}", held.checkout));
        if std::env::var_os(PREFIXED).is_some() {
            // On both streams: each is read by a thread of its own, and a line
            // after the marker in a stream must find the new prefix.
            println!("{CHECKOUT_MARKER}{}", held.checkout);
            eprintln!("{CHECKOUT_MARKER}{}", held.checkout);
        }
        let built_file = places.app_built_file(repo, &held.checkout);
        if let Site::Remote(remote) = &site {
            let folder = remote.local_folder();
            std::fs::create_dir_all(&folder)
                .context(|| format!("creating {}", folder.display()))?;
        }
        if kept_shots {
            note(
                "the picture folder keeps every shot these runs take for the lane's app tree, so the lane builds nothing and takes the shots from it",
            );
            Built::clear(&built_file)?;
        } else if names.is_empty() && !picks.is_empty() {
            let (wanted, head) = site.resolved_and_head(places, repo, commit)?;
            Built::allows_class_run(
                Built::read(&built_file)?.as_ref(),
                &held.checkout,
                commit,
                &wanted,
                &head,
            )?;
        } else {
            // The switch and the steps change what the checkout holds.
            Built::clear(&built_file)?;
        }
        let CheckedOut { sha, config } = site.check_out(places, repo, commit)?;
        timing.commit = Some(sha.clone());
        let program_of = |class: &str| {
            let program = crate::derive::program_of(&source, &config, commit, lane, class)?;
            if let Some(program) = &program {
                note(format!(
                    "{class} is declared in the {program} sources, so the {} test line for {program} runs it",
                    lane.word()
                ));
            }
            Ok(program)
        };
        let steps = lane_steps(&config, lane, names, picks, &program_of)?;
        if steps.is_empty() {
            fail!("{CONFIG} names no step in the {} lane", lane.word());
        }
        let (tests, builds): (Vec<Step>, Vec<Step>) = steps
            .into_iter()
            .partition(|step| pick_kind_of(&step.name).is_some());
        let shots = ShotsStore::of(
            places, &source, repo, lane, &sha, &config, picks, kept_shots,
        )?;
        let with = |results| StepsWith {
            results,
            shots: &shots,
            programs,
            watch: lane.uses_xcode(),
        };
        let results = config.results_of(lane);
        let ran = (|| {
            let mut ended = site.run_steps(&held, &builds, &with(&[]), &mut timing.steps)?;
            if ended == Ended::Finished && !builds.is_empty() {
                let steps = builds.iter().map(|step| step.name.clone()).collect();
                Built {
                    commit: site.head()?,
                    steps,
                }
                .write(&built_file)?;
            }
            if ended == Ended::Finished {
                ended = site.run_steps(&held, &tests, &with(&results), &mut timing.steps)?;
            }
            Ok(ended)
        })();
        match left_unchanged(&mut site, &built_file, ran)? {
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
        held = held.requeue()?;
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

/// Prints the stored findings of each `--shots` pick in <picks> that has a
/// run stored for <git_ref>'s commit, and returns the picks left to run: the
/// `--test` picks, and the `--shots` picks with no stored run. A run of picks
/// with no step builds nothing, so a stored run is printed in place of taking
/// the lane.
pub fn serve_stored(
    places: &Places,
    repo: &str,
    git_ref: &str,
    picks: Vec<Pick>,
) -> Result<Vec<Pick>> {
    if !picks.iter().any(|(_, kind, _)| *kind == PickKind::Shots) {
        return Ok(picks);
    }
    let mut left = Vec::new();
    for pick in picks {
        let (lane, kind, prefix) = &pick;
        if *kind != PickKind::Shots {
            left.push(pick);
            continue;
        }
        let key = shot_findings::key_for(places, repo, git_ref, *lane, prefix)?;
        match shot_store::lookup(places, &key)? {
            Some(stored) => {
                note("printing the stored findings instead of running (--fresh runs)");
                shot_findings::print(&stored);
                shot_findings::note_report(&stored);
            }
            None => left.push(pick),
        }
    }
    Ok(left)
}

/// Makes the store hold the run each `--shots` pick of <picks> compares
/// with: the run of <git_ref>'s merge base with origin/main, by the
/// shots checker this builds keys runs on (shot_store.rs). For the picks
/// with none, it builds the merge base first in their lanes, with each lane's
/// `needs` steps, and runs the picks there, which keeps their runs; the run
/// of a commit on origin/main compares with none, so its pairs' findings
/// fail it, and that failure stops nothing here. Fails when a pick still
/// has no run to compare with, or a stop signal ended the build. A commit on
/// origin/main is its own merge base and needs no before run. The merge
/// base's lanes run from <pinned>, the build's programs.
pub fn ensure_before_runs(
    places: &Places,
    pinned: &Pinned,
    repo: &str,
    git_ref: &str,
    picks: &[Pick],
) -> Result<()> {
    let shots: Vec<Pick> = picks
        .iter()
        .filter(|(_, kind, _)| *kind == PickKind::Shots)
        .cloned()
        .collect();
    if shots.is_empty() {
        return Ok(());
    }
    let source = local_source(places, repo)?;
    let commit = resolve(&source, git_ref)?;
    let base = crate::derive::merge_base(&source, &commit)?;
    if base == commit {
        return Ok(());
    }
    let version = crate::shots_program::VERSION;
    let missing = |picks: &[Pick]| -> Result<Vec<Pick>> {
        let mut missing = Vec::new();
        for pick in picks {
            let (lane, _, prefix) = pick;
            if shot_store::before_run(places, repo, &base, *lane, prefix, version)?.is_none() {
                missing.push(pick.clone());
            }
        }
        Ok(missing)
    };
    let short = &base[..base.len().min(12)];
    let wanted = missing(&shots)?;
    if wanted.is_empty() {
        note(format!(
            "the shots runs compare with the runs of {short}, the merge base with origin/main, that builds keeps"
        ));
        return Ok(());
    }
    note(format!(
        "builds keeps no run of {} for {short}, the merge base with origin/main, by shots checker {version}: building {short} and running them first, to compare with",
        wanted
            .iter()
            .map(|(_, _, prefix)| prefix.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let config = config_at(places, repo, &base)?;
    let lanes = crate::derive::plan(&config, &[], &wanted);
    let built = run_lanes(pinned, repo, &base, &lanes, &wanted, &[]);
    if PASSED_ON.load(Ordering::SeqCst) != 0 {
        return built;
    }
    let still = missing(&wanted)?;
    if let Some((lane, _, prefix)) = still.first() {
        let why = match built {
            Err(error) => error.text(),
            Ok(()) => "it kept no findings".to_string(),
        };
        fail!(
            "the run of {prefix} on {} for {short}, the merge base with origin/main, gave nothing to compare with: {why}",
            lane.word()
        );
    }
    if let Err(error) = built {
        note(format!(
            "the build of {short} failed ({}), and builds kept the findings of its shots runs, which the runs of {git_ref} compare with",
            error.text()
        ));
    }
    Ok(())
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
        if text.contains("builds: ") && !text.contains("kept ") {
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
    lane: &'static str,
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
        if let Some(named) = prefix_from_marker(lane, &line) {
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

/// Runs the lanes in <lanes> at the same time, each as a process of its own
/// from the folder <pinned>; fails, naming the lanes, if any does. A stop signal goes on to each lane,
/// and the command, once they have ended, fails saying it stopped.
pub fn run_lanes(
    pinned: &Pinned,
    repo: &str,
    commit: &str,
    lanes: &[(Lane, Vec<String>)],
    picks: &[Pick],
    kept: &[Lane],
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
            .args(["app-build", repo, "--lane", lane.word(), commit])
            .args(kept.contains(lane).then_some(KEPT_SHOTS))
            .args(names)
            .args(
                picks
                    .iter()
                    .filter(|(own, _, _)| own == lane)
                    .flat_map(pick_words),
            )
            .env(PREFIXED, "1")
            .env("PATH", pinned.path(std::env::var_os("PATH").as_deref()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
        let (word, out_prefix) = (lane.word(), prefix.clone());
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
            copy_lines(stderr, std::io::stderr(), word, prefix, err_tail, err_done)
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
    match argv {
        [_, command, _repo, flag, _lane, commit, ..]
            if command == "app-build" && flag == "--lane" && !commit.starts_with('-') =>
        {
            Some(commit.as_str())
        }
        _ => None,
    }
}

/// Returns the repo and the ref (the branch or commit to build) from the
/// command line <argv> of a running `builds app-build <repo> <ref> ...`
/// request. Returns None for any other command, for `--list`, and for a
/// lane's process (`--lane`), which the request starts and waits for.
pub fn requested(argv: &[String]) -> Option<(&str, &str)> {
    match argv {
        [_, command, repo, reference, ..]
            if command == "app-build" && !reference.starts_with('-') =>
        {
            Some((repo.as_str(), reference.as_str()))
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
        return crate::app_landing::cmd_landing_build(places, rest);
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
            let place = match picker.place {
                lock::Where::Anywhere => String::new(),
                place => format!(" (runs only {})", place.only()),
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
        let kept_shots = names.first().is_some_and(|arg| arg == KEPT_SHOTS);
        let names = if kept_shots { &names[1..] } else { names };
        let (names, picks) = split_picks(names)?;
        let lane = Lane::parse_or_fail(word)?;
        // A lane that its build started runs from the build's folder, which
        // holds its programs already; a lane run by hand copies them there.
        // The lane copies the folder to each host it runs a step on.
        let pinned = pin(places)?;
        // A lane's process always says why it ended badly: a bare exit status
        // from a path that noted nothing would reach the parent as no output.
        let request = LaneRequest {
            commit,
            names: &names,
            picks: &picks,
            kept_shots,
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
    if tail.first().is_none_or(|arg| arg.starts_with("--")) {
        // No step named: the diff chooses them (derive.rs).
        return crate::derive::derive_and_run(places, args);
    }
    let fresh = tail.iter().any(|arg| arg == "--fresh");
    let tail: Vec<String> = tail
        .iter()
        .filter(|arg| *arg != "--fresh")
        .cloned()
        .collect();
    let (names, mut picks) = split_picks(&tail)?;
    let names = &names[..];
    let config = config_at(places, repo, commit)?;
    for name in names {
        step_named(&config, name)?;
    }
    for (lane, kind, arg) in &picks {
        if config.picker(*lane, *kind).is_none() {
            return Err(missing_line(*lane, *kind, arg));
        }
    }
    if names.is_empty() && !picks.is_empty() && !fresh {
        picks = serve_stored(places, repo, commit, picks)?;
        if picks.is_empty() {
            return Ok(());
        }
    }
    let pinned = pin(places)?;
    ensure_before_runs(places, &pinned, repo, commit, &picks)?;
    // The lanes the steps or picks are in: those with a step named, or with
    // any step when none is named and no pick is, each with its own names.
    let lanes: Vec<(Lane, Vec<String>)> = Lane::ALL
        .into_iter()
        .map(|lane| (lane, names_in(&config, lane, names)))
        .filter(|(lane, own)| {
            let picks_here = picks.iter().any(|(pick_lane, _, _)| pick_lane == lane);
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
    run_lanes(&pinned, repo, commit, &lanes, &picks, &[])
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
