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

use crate::app_config::{
    CONFIG, Config, PickKind, Picker, Step, app_checkout_top, check_out, parse_config,
    run_command_keeping, run_git, show_duration,
};
use crate::app_remote::{RemoteApp, local_source};
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
use crate::shot_findings;
use crate::text::note;

const USAGE: &str = "usage: builds app-build <repo> <ref> [--test <Class>]... [--shots <prefix>]... [--fresh] | builds app-build <repo> <ref> <step>... [--test mac|android <Class>]... [--shots mac|android <prefix>]... [--fresh] | builds app-build <repo> --list | builds app-build <repo> --lane mac|android <ref> [<step>...] [--test mac|android <Class>]... [--shots mac|android <prefix>]...";

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
            "step {name} is CI-only in {CONFIG}, and builds refuses it: {ONLY_WHAT_A_CHANGE_TOUCHES}. Name the change's test classes with --test and its screens' shots with --shots"
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
/// the lane starts with `<lane>.`.
pub fn refuse_whole_lane(lane: Lane, prefix: &str) -> Result<()> {
    let word = lane.word();
    if format!("{word}.").starts_with(prefix) {
        fail!(
            "the shots prefix {prefix} selects every shot of the {word} lane, and builds refuses a whole lane's shots: {ONLY_WHAT_A_CHANGE_TOUCHES}. Give a prefix that names the screens the change touches, such as {word}.welcome. for the welcome screens"
        );
    }
    Ok(())
}

/// The kind of step <name> is, when it is the run of a pick.
fn pick_kind_of(name: &str) -> Option<PickKind> {
    PickKind::ALL
        .into_iter()
        .find(|kind| name.starts_with(kind.step_prefix()))
}

/// <args> (the steps, then `--test` and `--shots` flags, each with a lane and
/// an argument) as the step names and the flags.
fn split_picks(args: &[String]) -> Result<(Vec<String>, Vec<Pick>)> {
    let is_flag = |arg: &String| PickKind::ALL.iter().any(|kind| arg == kind.flag());
    let at = args.iter().position(is_flag).unwrap_or(args.len());
    let (names, mut flags) = (args[..at].to_vec(), &args[at..]);
    let mut picks = Vec::new();
    while let Some((flag, rest)) = flags.split_first() {
        let Some(kind) = PickKind::ALL.into_iter().find(|kind| flag == kind.flag()) else {
            fail!("{flag:?} where --test or --shots goes\n{USAGE}");
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
fn place_of(steps: &[Step]) -> Result<lock::Where> {
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

/// The full id of <rev> in the checkout <top>.
fn resolve(top: &Path, rev: &str) -> Result<String> {
    Ok(run_git(
        top,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )?
    .trim()
    .to_string())
}

/// How a lane's steps ended, when none failed because of the code.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    Finished,
    /// A takeover stopped a step.
    TakenOver,
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

/// What a step's run keeps of its output: all of it for a shots run with a
/// store that keeps its findings (<all>), and the first line saying the host
/// failed the step when the lane watches for one (<fault>).
fn keep_for<'a>(
    kind: StepKind,
    shots: Option<&ShotsStore>,
    all: &'a lock::Captured,
    fault: Option<&'a Found>,
) -> Keep<'a> {
    Keep {
        all: (kind == StepKind::Shots && shots.is_some()).then_some(all),
        line_with: fault.map(|found| (HOST_FAULT_TEXTS, found)),
    }
}

/// Runs <step> (of <kind>) in the checkout <top>, watching for a line of
/// `HOST_FAULTS` in <fault>; a shots run with a store keeps the findings it printed.
fn run_local_step(
    top: &Path,
    step: &Step,
    kind: StepKind,
    shots: Option<&ShotsStore>,
    fault: Option<&Found>,
) -> Result<Within> {
    let kept = lock::Captured::default();
    let keep = keep_for(kind, shots, &kept, fault);
    let outcome = run_command_keeping(top, &step.command, Some(step.limit()), keep);
    if let (StepKind::Shots, Some(store)) = (kind, shots) {
        store.keep_of(&outcome, &kept, step, top, &mut LocalRun(top));
    }
    outcome
}

/// Runs <steps> in the checkout <top>, as `run_steps_with` does with <watch>.
fn run_steps(
    top: &Path,
    held: &lock::Held,
    steps: &[Step],
    log: &mut Vec<StepRecord>,
    shots: Option<&ShotsStore>,
    watch: bool,
) -> Result<Ended> {
    run_steps_with(held, steps, log, watch, &mut |step, kind, fault| {
        run_local_step(top, step, kind, shots, fault)
    })
}

/// Runs one step of a kind, watching for a line of `HOST_FAULTS` when given
/// where to keep it.
type RunStep<'a> = dyn FnMut(&Step, StepKind, Option<&Found>) -> Result<Within> + 'a;

/// Runs <steps> in order, each by <run>, stopping at the first that fails.
/// `run_steps_with` records each step as it starts in the record <held>
/// keeps beside its lock (holder_run.rs), and times each step it starts into
/// <log> (app_timing.rs). With <watch>, each step's run watches for a line of
/// `HOST_FAULTS`, and a step that fails after printing one ends the run with
/// `HostFault`.
fn run_steps_with(
    held: &lock::Held,
    steps: &[Step],
    log: &mut Vec<StepRecord>,
    watch: bool,
    run: &mut RunStep,
) -> Result<Ended> {
    for step in steps {
        note(format!("step {}", step.name));
        held.runs(&step.name)?;
        let kind = match pick_kind_of(&step.name) {
            Some(PickKind::Class) => StepKind::Test,
            Some(PickKind::Shots) => StepKind::Shots,
            None => StepKind::Step,
        };
        let timer = Timer::start();
        let found = Found::default();
        let outcome = run(step, kind, watch.then_some(&found));
        let result = match &outcome {
            Ok(Within::Ran(Ran::Finished)) => StepResult::Passed,
            Ok(Within::Ran(Ran::TakenOver)) => StepResult::TakenOver,
            Ok(Within::TimedOut) => StepResult::TimedOut,
            Err(_) => StepResult::Failed,
        };
        log.push(timer.finish(&step.name, kind, result));
        match outcome {
            Ok(Within::Ran(Ran::Finished)) => {}
            Ok(Within::Ran(Ran::TakenOver)) => return Ok(Ended::TakenOver),
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
                return Err(Fail::Exit(code));
            }
            Err(other) => return Err(other),
        }
    }
    Ok(Ended::Finished)
}

/// The repo's `.app-build` at <commit>, read without touching a checkout's
/// tree, to tell which lanes the steps are in before any lock is taken. A
/// lane's own run reads it again from its checkout, under its lock.
fn config_at(places: &Places, repo: &str, commit: &str) -> Result<Config> {
    let source = local_source(places, repo)?;
    run_git(&source, &["fetch", "-q", "origin"])?;
    parse_config(&run_git(&source, &["show", &format!("{commit}:{CONFIG}")])?)
}

/// Where a lane's steps run: in an app checkout on this Mac, or in one on a
/// host.
enum Site {
    Local(std::path::PathBuf),
    Remote(Box<RemoteApp>),
}

impl Site {
    /// The checkout <entry> of <repo>'s <lane>.
    fn of(places: &Places, ssh: &Ssh, repo: &str, lane: Lane, entry: &str) -> Result<Site> {
        Ok(match RemoteApp::find(places, ssh, repo, lane, entry)? {
            Some(remote) => Site::Remote(Box::new(remote)),
            None => Site::Local(app_checkout_top(places, repo, entry)?),
        })
    }

    /// The commit <commit> names, and the commit the checkout is on now.
    fn resolved_and_head(
        &mut self,
        places: &Places,
        repo: &str,
        commit: &str,
    ) -> Result<(String, String)> {
        match self {
            Site::Local(top) => {
                run_git(top, &["fetch", "-q", "origin"])?;
                Ok((resolve(top, commit)?, resolve(top, "HEAD")?))
            }
            Site::Remote(remote) => {
                let source = local_source(places, repo)?;
                run_git(&source, &["fetch", "-q", "origin"])?;
                Ok((resolve(&source, commit)?, remote.head()?))
            }
        }
    }

    /// The name of the host the checkout is on.
    fn host_name(&self) -> &str {
        match self {
            Site::Local(_) => crate::host::LOCAL,
            Site::Remote(remote) => &remote.host.name,
        }
    }

    /// The commit the checkout is on.
    fn head(&mut self) -> Result<String> {
        match self {
            Site::Local(top) => resolve(top, "HEAD"),
            Site::Remote(remote) => remote.head(),
        }
    }

    /// Switches the checkout to <commit> (which must have no changes) and
    /// reads the repo's `.app-build` there.
    fn check_out(&mut self, places: &Places, repo: &str, commit: &str) -> Result<Config> {
        match self {
            Site::Local(top) => check_out(top, commit),
            Site::Remote(remote) => {
                let source = local_source(places, repo)?;
                run_git(&source, &["fetch", "-q", "origin"])?;
                let sha = resolve(&source, commit)?;
                remote.switch_to(&source, &sha)?;
                parse_config(&run_git(&source, &["show", &format!("{sha}:{CONFIG}")])?)
            }
        }
    }

    /// Runs <steps> in the checkout, as `run_steps_with` does with <watch>;
    /// after each, a checkout on a host copies <results> back, and a shots run
    /// keeps its findings in <shots> here.
    fn run_steps(
        &mut self,
        held: &lock::Held,
        steps: &[Step],
        results: &[String],
        shots: Option<&ShotsStore>,
        watch: bool,
        log: &mut Vec<StepRecord>,
    ) -> Result<Ended> {
        match self {
            Site::Local(top) => run_steps(top, held, steps, log, shots, watch),
            Site::Remote(remote) => {
                run_steps_with(held, steps, log, watch, &mut |step, kind, fault| {
                    run_remote_step(remote, step, kind, results, shots, fault)
                })
            }
        }
    }
}

/// Runs <step> (of <kind>) in the checkout on a host, watching for a line of
/// `HOST_FAULTS` in <fault>, keeps the findings of a shots run here, then
/// copies <results> back, since a step that failed or ran out of time has
/// written the reports its reader needs most. A copy that fails fails a step
/// that passed; after a step that failed, the run notes the copy's error and
/// keeps the step's own result.
fn run_remote_step(
    remote: &mut RemoteApp,
    step: &Step,
    kind: StepKind,
    results: &[String],
    shots: Option<&ShotsStore>,
    fault: Option<&Found>,
) -> Result<Within> {
    let start = remote.step_command(&step.command)?;
    let kept = lock::Captured::default();
    let keep = keep_for(kind, shots, &kept, fault);
    let ran = lock::run_in_group_keeping(start, &step.command[0], Some(step.limit()), keep);
    if let (StepKind::Shots, Some(store)) = (kind, shots) {
        // The commit is the host's checkout's, which this Mac holds too.
        let source = remote.source()?;
        store.keep_of(&ran, &kept, step, &source, remote);
    }
    if results.is_empty() || matches!(ran, Ok(Within::Ran(Ran::TakenOver))) {
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
    let top = local_source(places, repo)?;
    run_git(&top, &["fetch", "-q", "origin"])?;
    let commit = resolve(&top, git_ref)?;
    let config = parse_config(&run_git(&top, &["show", &format!("{commit}:{CONFIG}")])?)?;
    Ok((top, commit, config))
}

/// Where a lane's run keeps the findings of its `--shots` runs.
struct ShotsStore {
    repo: String,
    lane: Lane,
    /// The folder of the shots program's source.
    source: String,
    database: std::path::PathBuf,
}

impl ShotsStore {
    /// The store for <repo>'s <lane> run, when the repo's config names the
    /// shots program's source and the repo is in a project with a store.
    fn of(places: &Places, repo: &str, lane: Lane, config: &Config) -> Option<ShotsStore> {
        let source = config.shots_source.clone()?;
        let database = match app_timing::database_of(places, repo) {
            Ok(Some(database)) => database,
            Ok(None) => return None,
            Err(error) => {
                note(format!("shot findings are not kept: {}", error.text()));
                return None;
            }
        };
        Some(ShotsStore {
            repo: repo.to_string(),
            lane,
            source,
            database,
        })
    }

    /// Keeps what the shots run <step> printed (<kept>), when it ended
    /// (<outcome>) with an exit status, as the findings of the commit
    /// <head> names, which <source> holds.
    fn keep_of(
        &self,
        outcome: &Result<Within>,
        kept: &lock::Captured,
        step: &Step,
        source: &Path,
        site: &mut dyn RunSite,
    ) {
        let exit = match outcome {
            Ok(Within::Ran(Ran::Finished)) => 0,
            Err(Fail::Exit(code)) => i64::from(*code),
            _ => return,
        };
        let prefix = step.name.strip_prefix(PickKind::Shots.step_prefix());
        let text = String::from_utf8_lossy(&kept.lock().expect("no panic holding it")).into_owned();
        self.keep(source, site, prefix.unwrap_or(&step.name), exit, &text);
    }

    /// Keeps what a run of <prefix> that exited <exit> printed in <output>,
    /// as the findings of the commit the site is on, found in <source>, and
    /// copies the report it names from the site (shot_findings.rs). A run
    /// that printed no findings, or a write or copy that fails, is said and
    /// changes nothing about the build's result.
    fn keep(&self, source: &Path, site: &mut dyn RunSite, prefix: &str, exit: i64, output: &str) {
        let findings = shot_findings::findings_of(output);
        if !shot_findings::has_findings(exit, findings) {
            note(format!(
                "the shots run of {prefix} exited {exit} without findings, so none were kept"
            ));
            return;
        }
        let stored = site.commit().and_then(|commit| {
            let key = shot_findings::Key {
                repo: self.repo.clone(),
                version: shot_findings::version_at(source, &commit, &self.source)?,
                commit,
                lane: self.lane,
                prefix: prefix.to_string(),
            };
            shot_findings::store(&self.database, &key, exit, findings)?;
            Ok(key)
        });
        let key = match stored {
            Ok(key) => key,
            Err(error) => {
                note(format!("the shot findings were not kept: {}", error.text()));
                return;
            }
        };
        let Some(dir) = shot_findings::report_dir_in(output) else {
            note(format!(
                "the shots run of {prefix} printed no report path, so no report was kept"
            ));
            return;
        };
        let folder = shot_findings::report_folder(&self.database, &key);
        match site.fetch_report(&dir, &folder) {
            Ok(()) => {
                note(format!(
                    "shots report for {prefix}: {}",
                    folder.join("index.html").display()
                ));
                if let Some(shots) = folder.parent() {
                    shot_findings::prune_reports(shots);
                }
            }
            Err(error) => note(format!("the shots report was not kept: {}", error.text())),
        }
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
    /// The use of the stores the lane takes each time it holds the lane's
    /// lock (`hold_stores`), held until `record` has written the run or the
    /// lane goes back to the queue.
    stores: Option<install_gate::Holding<'static>>,
}

/// When a run joined the lane's queue, how long it waited there in all, and
/// the checkout whose lock it holds, as `lock::Held` keeps them.
struct HeldTimes {
    joined_at: i64,
    wait: Duration,
    checkout: String,
}

impl HeldTimes {
    fn of(held: &lock::Held) -> HeldTimes {
        HeldTimes {
            joined_at: held.joined_at,
            wait: held.waited,
            checkout: held.checkout.clone(),
        }
    }
}

impl Timing {
    fn start() -> Timing {
        Timing {
            held: None,
            steps: Vec::new(),
            stores: None,
        }
    }

    /// Writes the run to the repo's project store when it took a checkout. A
    /// repo in no project, or a write that fails, is said and changes nothing
    /// about the build's result.
    fn record(self, places: &Places, repo: &str, lane: Lane, built_ref: &str, passed: bool) {
        let Some(held) = self.held else {
            return;
        };
        let written = app_timing::database_of(places, repo).and_then(|database| {
            let Some(database) = database else {
                return Ok(());
            };
            app_timing::record(
                &database,
                &RunRecord {
                    started_at: held.joined_at,
                    repo: repo.to_string(),
                    lane,
                    checkout: held.checkout,
                    built_ref: built_ref.to_string(),
                    who: who_asked(),
                    wait_ms: held.wait.as_millis() as u64,
                    passed,
                    steps: self.steps,
                },
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

/// Builds <commit> in <lane>'s app checkout on the lane's lock: <names> are
/// the lane's steps to run, or all of them when none and no <picks>. The run
/// and its steps are timed and recorded (app_timing.rs). A lane that adds a
/// checkout on a host keeps the host's free disk above <floor_kib>.
fn run_lane(
    places: &Places,
    floor_kib: u64,
    repo: &str,
    lane: Lane,
    commit: &str,
    names: &[String],
    picks: &[Pick],
) -> Result<()> {
    let mut timing = Timing::start();
    let result = build_lane(
        places,
        floor_kib,
        repo,
        lane,
        commit,
        names,
        picks,
        &mut timing,
    );
    let passed = result.is_ok();
    timing.record(places, repo, lane, commit, passed);
    result
}

/// Takes the use of the stores a lane holds while it holds the lane's lock
/// <held> (install_gate.rs), into <stores>. When install ran while the lane
/// waited, the lane's program writes a store of another schema, so the lane
/// leaves the queue and runs the installed builds in its place.
fn hold_stores(
    held: lock::Held,
    stores: &mut Option<install_gate::Holding<'static>>,
) -> Result<lock::Held> {
    *stores = install_gate::hold()?;
    if install_gate::install_ran()? {
        note(
            "builds install ran while this build waited for the lane, so this build leaves the queue and runs the installed builds program, which joins the queue again",
        );
        held.leave()?;
        return Err(install_gate::run_again());
    }
    Ok(held)
}

#[allow(clippy::too_many_arguments)]
fn build_lane(
    places: &Places,
    floor_kib: u64,
    repo: &str,
    lane: Lane,
    commit: &str,
    names: &[String],
    picks: &[Pick],
    timing: &mut Timing,
) -> Result<()> {
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
    let held = lock::take_where(places, &ssh, repo, lane, allowed, floor_kib)?;
    let mut held = hold_stores(held, &mut timing.stores)?;
    loop {
        timing.held = Some(HeldTimes::of(&held));
        let mut site = Site::of(places, &ssh, repo, lane, &held.checkout)?;
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
        if names.is_empty() && !picks.is_empty() {
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
        let config = site.check_out(places, repo, commit)?;
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
        let shots = ShotsStore::of(places, repo, lane, &config);
        let watch = lane.uses_xcode();
        let mut ended = site.run_steps(
            &held,
            &builds,
            &[],
            shots.as_ref(),
            watch,
            &mut timing.steps,
        )?;
        if ended == Ended::Finished && !builds.is_empty() {
            let steps = builds.iter().map(|step| step.name.clone()).collect();
            Built {
                commit: site.head()?,
                steps,
            }
            .write(&built_file)?;
        }
        if ended == Ended::Finished {
            ended = site.run_steps(
                &held,
                &tests,
                &config.results_of(lane),
                shots.as_ref(),
                watch,
                &mut timing.steps,
            )?;
        }
        match ended {
            Ended::Finished => return Ok(()),
            Ended::TakenOver => {
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
        // A build that waits in the queue writes nothing, so the lane ends its
        // use until it holds a lock again, and an install that runs meanwhile
        // waits for no queued build.
        timing.stores = None;
        held = hold_stores(held.requeue()?, &mut timing.stores)?;
    }
}

/// Prints the stored findings of each `--shots` pick in <picks> that has a
/// run stored for <git_ref>'s commit, and returns the picks left to run: the
/// `--test` picks, and the `--shots` picks with no stored run (a repo with no
/// `shots-source` line stores none). A run of picks with no step builds
/// nothing, so a stored run is printed in place of taking the lane.
pub fn serve_stored(
    places: &Places,
    repo: &str,
    git_ref: &str,
    config: &Config,
    picks: Vec<Pick>,
) -> Result<Vec<Pick>> {
    if config.shots_source.is_none() || !picks.iter().any(|(_, kind, _)| *kind == PickKind::Shots) {
        return Ok(picks);
    }
    let mut left = Vec::new();
    for pick in picks {
        let (lane, kind, prefix) = &pick;
        if *kind != PickKind::Shots {
            left.push(pick);
            continue;
        }
        let (key, database) = shot_findings::key_for(places, repo, git_ref, *lane, prefix)?;
        let stored = match &database {
            Some(database) => shot_findings::lookup(database, &key)?,
            None => None,
        };
        match stored {
            Some(stored) => {
                note("printing the stored findings instead of running (--fresh runs)");
                shot_findings::print(&stored);
                if let Some(database) = &database {
                    shot_findings::note_report(database, &stored.key);
                }
            }
            None => left.push(pick),
        }
    }
    Ok(left)
}

/// The pids of the lanes' processes, for the signal handler to pass on to.
static LANE_PIDS: [AtomicI32; 2] = [AtomicI32::new(0), AtomicI32::new(0)];

extern "C" fn pass_to_lanes(signal: libc::c_int) {
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

/// Runs the lanes in <lanes> at the same time, each as a process of its own;
/// fails, naming the lanes, if any does.
pub fn run_lanes(
    repo: &str,
    commit: &str,
    lanes: &[(Lane, Vec<String>)],
    picks: &[Pick],
) -> Result<()> {
    let exe = std::env::current_exe().context(|| "finding the builds program".to_string())?;
    unsafe {
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(signal, pass_to_lanes as *const () as libc::sighandler_t);
        }
    }
    let mut running = Vec::new();
    for (at, (lane, names)) in lanes.iter().enumerate() {
        let mut command = Command::new(&exe);
        command
            .args(["app-build", repo, "--lane", lane.word(), commit])
            .args(names)
            .args(
                picks
                    .iter()
                    .filter(|(own, _, _)| own == lane)
                    .flat_map(|(own, kind, arg)| [kind.flag(), own.word(), arg.as_str()]),
            )
            .env(PREFIXED, "1")
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
        let mut message = format!("app-build failed in {}", failed.join(" and "));
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

/// `builds app-build`. <floor_kib> is the free-disk floor a host keeps when
/// a lane adds a checkout there (lane_size.rs).
pub fn cmd_app_build(places: &Places, floor_kib: u64, args: &[String]) -> Result<()> {
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
        let (names, picks) = split_picks(names)?;
        let lane = Lane::parse_or_fail(word)?;
        // A lane's process always says why it ended badly: a bare exit status
        // from a path that noted nothing would reach the parent as no output.
        return match run_lane(places, floor_kib, repo, lane, commit, &names, &picks) {
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
        picks = serve_stored(places, repo, commit, &config, picks)?;
        if picks.is_empty() {
            return Ok(());
        }
    }
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
    run_lanes(repo, commit, &lanes, &picks)
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
