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
//! so in a marker line the parent reads and does not print). Without a step named it runs every step, in the
//! order written; with steps named, each lane runs its own in the order
//! named. It exits 1 if either lane fails, saying which.
//!
//! A lane's process takes the first free lock among the lane's app checkouts
//! (queue.rs: first come, first served across the set), checks <ref> out in
//! that checkout (which must be clean) and runs the lane's steps each through
//! `run_in_group`, stopping at the first one that fails. A step has a time
//! limit (app_config.rs): past it the step's process group is killed and the
//! build fails, which gives up the lock.
//! `builds app-build <repo> --list` prints the steps and the test lines.
//!
//! `--test <lane> <Class>` (any number, after the steps) adds, to the end of
//! that lane's steps, the lane's `test` line (app_config.rs) with the class in
//! it, one run per class, each with `TEST_CLASS_LIMIT`. With `--test` and no
//! step named, only the classes run, not every step.
//!
//! A class run with no step named tests what the checkout holds, so it
//! refuses unless the checkout's last build (`Built`, kept beside the lock's
//! checkout folder, written when a run's steps all finish and removed when a
//! run with steps starts) is of the commit it names and the checkout is still
//! on that commit; it names what was built instead.
//!
//! A takeover stops the step running; the lane's build then runs again from
//! the checkout, behind the build that took over.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use crate::app_config::{
    CONFIG, Config, Step, app_checkout_top, check_out, parse_config, run_command, run_git, show_duration,
};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lane::Lane;
use crate::lock::{self, Ran, Within};
use crate::places::Places;
use crate::text::note;

const USAGE: &str = "usage: builds app-build <repo> <ref> [<step>...] [--test mac|android <Class>]... | builds app-build <repo> --list | builds app-build <repo> --lane mac|android <ref> [<step>...] [--test mac|android <Class>]...";

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

/// The steps of <lane> to run: those named that are in it, in the order
/// named, or all of the lane's in the order written when none is named.
fn chosen(config: &Config, lane: Lane, names: &[String]) -> Result<Vec<Step>> {
    if names.is_empty() {
        return Ok(config.steps.iter().filter(|step| step.lane == lane).cloned().collect());
    }
    names
        .iter()
        .map(|name| match config.steps.iter().find(|step| step.name == *name) {
            Some(step) if step.lane == lane => Ok(step.clone()),
            Some(step) => fail!("step {name} is in the {} lane, not the {} lane", step.lane.word(), lane.word()),
            None => {
                let known: Vec<&str> = config.steps.iter().map(|step| step.name.as_str()).collect();
                fail!("{CONFIG} has no step {name} (it has: {})", known.join(", "))
            }
        })
        .collect()
}

/// How long a test class may run, as the limit of a step; the time the
/// per-class steps in bae's `.app-build` had.
pub const TEST_CLASS_LIMIT: Duration = Duration::from_secs(10 * 60);

/// The name <class>'s run goes by in messages and the dashboard's step lines.
const TEST_STEP_PREFIX: &str = "test ";

/// A `--test <lane> <Class>` flag.
type ClassFlag = (Lane, String);

/// <args> (the steps, then `--test <lane> <Class>` flags) as the step names
/// and the lane and class of each flag.
fn split_tests(args: &[String]) -> Result<(Vec<String>, Vec<ClassFlag>)> {
    let at = args.iter().position(|arg| arg == "--test").unwrap_or(args.len());
    let (names, mut flags) = (args[..at].to_vec(), &args[at..]);
    let mut tests = Vec::new();
    while let Some((flag, rest)) = flags.split_first() {
        let [lane, class, after @ ..] = rest else {
            fail!("--test needs a lane and a class\n{USAGE}");
        };
        if flag != "--test" {
            fail!("{flag:?} where --test <lane> <Class> goes\n{USAGE}");
        }
        if class.is_empty() || class.starts_with('-') || class.contains(char::is_whitespace) {
            fail!("--test {} takes one class name, not {class:?}", lane);
        }
        tests.push((Lane::parse_or_fail(lane)?, class.clone()));
        flags = after;
    }
    Ok((names, tests))
}

/// What <lane> runs: its steps (<names>, or every one when none is named and
/// no class is), then its `test` line for each of <classes> that is the lane's.
fn lane_steps(config: &Config, lane: Lane, names: &[String], classes: &[ClassFlag]) -> Result<Vec<Step>> {
    let mut steps =
        if names.is_empty() && !classes.is_empty() { Vec::new() } else { chosen(config, lane, names)? };
    for (_, class) in classes.iter().filter(|(own, _)| *own == lane) {
        let Some(test) = config.class_test(lane) else {
            fail!("{CONFIG} has no `test {} ...` line, so it cannot run the class {class}", lane.word());
        };
        steps.push(Step {
            name: format!("{TEST_STEP_PREFIX}{class}"),
            lane,
            command: test.command_for(class),
            limit: Some(TEST_CLASS_LIMIT),
        });
    }
    Ok(steps)
}

/// The names in <names> that are steps of <lane> in <config>, in order.
fn names_in(config: &Config, lane: Lane, names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| config.steps.iter().any(|step| step.name == **name && step.lane == lane))
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
        let steps = lines.next().unwrap_or("").split_whitespace().map(str::to_string).collect();
        Ok(Some(Built { commit: commit.to_string(), steps }))
    }

    /// Written in one rename, so a reader never sees half of it.
    fn write(&self, file: &Path) -> Result<()> {
        let part = file.with_extension("part");
        std::fs::write(&part, format!("{}\n{}\n", self.commit, self.steps.join(" ")))
            .context(|| format!("writing {}", part.display()))?;
        std::fs::rename(&part, file).context(|| format!("replacing {}", file.display()))
    }

    fn clear(file: &Path) -> Result<()> {
        match std::fs::remove_file(file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => fail!("removing {}: {error}", file.display()),
            _ => Ok(()),
        }
    }

    /// Whether a class run of <wanted> (named <asked> by the caller) may go
    /// ahead in <checkout>, which is on <head> and last built <built>.
    fn allows_class_run(built: Option<&Built>, checkout: &str, asked: &str, wanted: &str, head: &str) -> Result<()> {
        let hint = "name the steps that build it (a class-only run tests what the checkout holds)";
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
    Ok(run_git(top, &["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?.trim().to_string())
}

/// Runs <steps> in the checkout <top> in order, stopping at the first that
/// fails; `TakenOver` when a takeover stopped one.
fn run_steps(top: &Path, steps: &[Step]) -> Result<Ran> {
    for step in steps {
        note(format!("step {}", step.name));
        match run_command(top, &step.command, Some(step.limit())) {
            Ok(Within::Ran(Ran::Finished)) => {}
            Ok(Within::Ran(Ran::TakenOver)) => return Ok(Ran::TakenOver),
            Ok(Within::TimedOut) => {
                let hint = if step.name.starts_with(TEST_STEP_PREFIX) {
                    String::new()
                } else {
                    format!(" (a `limit {} <time>` line in {CONFIG} sets another)", step.name)
                };
                fail!(
                    "step {} ran past its limit of {} and its process group was killed; the lock goes to the next build{hint}",
                    step.name,
                    show_duration(step.limit()),
                );
            }
            Err(Fail::Exit(code)) => {
                note(format!("step {} failed (exit {code})", step.name));
                return Err(Fail::Exit(code));
            }
            Err(other) => return Err(other),
        }
    }
    Ok(Ran::Finished)
}

/// The first app checkout recorded for <repo>, in the first lane that has
/// one, and its folder.
fn any_checkout(places: &Places, repo: &str) -> Result<std::path::PathBuf> {
    for lane in Lane::ALL {
        if let Some(name) = places.app_checkouts_of(repo, lane)?.first() {
            return app_checkout_top(places, repo, name);
        }
    }
    fail!("{repo} has no app checkout; builds app-checkout records one")
}

/// The repo's `.app-build` at <commit>, read without touching a checkout's
/// tree, to tell which lanes the steps are in before any lock is taken. A
/// lane's own run reads it again from its checkout, under its lock.
fn config_at(places: &Places, repo: &str, commit: &str) -> Result<Config> {
    let top = any_checkout(places, repo)?;
    run_git(&top, &["fetch", "-q", "origin"])?;
    parse_config(&run_git(&top, &["show", &format!("{commit}:{CONFIG}")])?)
}

/// Builds <commit> in <lane>'s app checkout on the lane's lock: <names> are
/// the lane's steps to run, or all of them when none and no <classes>.
fn run_lane(
    places: &Places,
    repo: &str,
    lane: Lane,
    commit: &str,
    names: &[String],
    classes: &[ClassFlag],
) -> Result<()> {
    note(format!("waiting for {repo}'s {} app lock, then building {commit}", lane.word()));
    let mut held = lock::take(places, repo, lane)?;
    loop {
        let top = app_checkout_top(places, repo, &held.checkout)?;
        note(format!("building in {}", held.checkout));
        if std::env::var_os(PREFIXED).is_some() {
            // On both streams: each is read by a thread of its own, and a line
            // after the marker in a stream must find the new prefix.
            println!("{CHECKOUT_MARKER}{}", held.checkout);
            eprintln!("{CHECKOUT_MARKER}{}", held.checkout);
        }
        let built_file = places.app_built_file(repo, &held.checkout);
        if names.is_empty() && !classes.is_empty() {
            run_git(&top, &["fetch", "-q", "origin"])?;
            let (wanted, head) = (resolve(&top, commit)?, resolve(&top, "HEAD")?);
            Built::allows_class_run(Built::read(&built_file)?.as_ref(), &held.checkout, commit, &wanted, &head)?;
        } else {
            // The switch and the steps change what the checkout holds.
            Built::clear(&built_file)?;
        }
        let config = check_out(&top, commit)?;
        let steps = lane_steps(&config, lane, names, classes)?;
        if steps.is_empty() {
            fail!("{CONFIG} names no step in the {} lane", lane.word());
        }
        let (tests, builds): (Vec<Step>, Vec<Step>) =
            steps.into_iter().partition(|step| step.name.starts_with(TEST_STEP_PREFIX));
        let mut ran = run_steps(&top, &builds)?;
        if ran == Ran::Finished && !builds.is_empty() {
            let steps = builds.iter().map(|step| step.name.clone()).collect();
            Built { commit: resolve(&top, "HEAD")?, steps }.write(&built_file)?;
        }
        if ran == Ran::Finished {
            ran = run_steps(&top, &tests)?;
        }
        match ran {
            Ran::Finished => return Ok(()),
            Ran::TakenOver => {
                note(format!("the build was stopped for a takeover; building {commit} again after it"));
                held = held.requeue()?;
            }
        }
    }
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

/// Copies <from> to <to>, each line behind <prefix>; sends on <done> at the
/// end.
fn copy_lines(
    from: impl Read,
    mut to: impl Write,
    lane: &'static str,
    prefix: Arc<Mutex<String>>,
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
        let mut out = prefix.lock().expect("no panic holding it").clone().into_bytes();
        out.extend_from_slice(&line);
        if !line.ends_with(b"\n") {
            out.push(b'\n');
        }
        let _ = to.write_all(&out);
        line.clear();
    }
    let _ = done.send(());
}

/// Runs the lanes in <lanes> at the same time, each as a process of its own;
/// fails, naming the lanes, if any does.
fn run_lanes(repo: &str, commit: &str, lanes: &[(Lane, Vec<String>)], classes: &[ClassFlag]) -> Result<()> {
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
            .args(classes.iter().filter(|(own, _)| own == lane).flat_map(|(own, class)| ["--test", own.word(), class]))
            .env(PREFIXED, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context(|| format!("starting the {} lane's build", lane.word()))?;
        LANE_PIDS[at].store(child.id() as i32, Ordering::SeqCst);
        let (done, ended) = channel();
        // Both streams share it: it changes once the lane's process says which
        // checkout it builds in.
        let prefix = Arc::new(Mutex::new(format!("[{}] ", lane.word())));
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let (out_done, err_done) = (done.clone(), done);
        let (word, out_prefix) = (lane.word(), prefix.clone());
        std::thread::spawn(move || copy_lines(stdout, std::io::stdout(), word, out_prefix, out_done));
        std::thread::spawn(move || copy_lines(stderr, std::io::stderr(), word, prefix, err_done));
        running.push((*lane, child, ended));
    }
    let mut failed = Vec::new();
    for (lane, mut child, ended) in running {
        let status = child.wait().context(|| format!("waiting for the {} lane's build", lane.word()))?;
        for _ in 0..2 {
            if ended.recv_timeout(OUTPUT_GRACE) == Err(RecvTimeoutError::Timeout) {
                break;
            }
        }
        if !status.success() {
            failed.push(match status.code() {
                Some(code) => format!("the {} lane (exit {code})", lane.word()),
                None => format!("the {} lane ({status})", lane.word()),
            });
        }
    }
    if failed.is_empty() { Ok(()) } else { fail!("app-build failed in {}", failed.join(" and ")) }
}

pub fn cmd_app_build(places: &Places, args: &[String]) -> Result<()> {
    let [repo, rest @ ..] = args else {
        fail!("{USAGE}");
    };
    if let [flag] = rest
        && flag == "--list"
    {
        let top = any_checkout(places, repo)?;
        let file = top.join(CONFIG);
        let text = std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
        let config = parse_config(&text)?;
        for step in &config.steps {
            println!("{} {} {}", step.lane.word(), step.name, step.command.join(" "));
        }
        for test in &config.tests {
            println!("{} --test <Class> {}", test.lane.word(), test.command.join(" "));
        }
        return Ok(());
    }
    if let [flag, word, commit, names @ ..] = rest
        && flag == "--lane"
        && !commit.starts_with('-')
    {
        let (names, classes) = split_tests(names)?;
        return run_lane(places, repo, Lane::parse_or_fail(word)?, commit, &names, &classes);
    }
    let [commit, tail @ ..] = rest else {
        fail!("{USAGE}");
    };
    if commit.starts_with('-') {
        fail!("{USAGE}");
    }
    let (names, classes) = split_tests(tail)?;
    let names = &names[..];
    let config = config_at(places, repo, commit)?;
    for name in names {
        if !config.steps.iter().any(|step| step.name == *name) {
            let known: Vec<&str> = config.steps.iter().map(|step| step.name.as_str()).collect();
            fail!("{CONFIG} has no step {name} (it has: {})", known.join(", "));
        }
    }
    for (lane, class) in &classes {
        if config.class_test(*lane).is_none() {
            fail!("{CONFIG} has no `test {} ...` line, so it cannot run the class {class}", lane.word());
        }
    }
    // The lanes the steps or classes are in: those with a step named, or with
    // any step when none is named and no class is, each with its own names.
    let lanes: Vec<(Lane, Vec<String>)> = Lane::ALL
        .into_iter()
        .map(|lane| (lane, names_in(&config, lane, names)))
        .filter(|(lane, own)| {
            let classes_here = classes.iter().any(|(class_lane, _)| class_lane == lane);
            if names.is_empty() && classes.is_empty() {
                config.steps.iter().any(|step| step.lane == *lane)
            } else {
                !own.is_empty() || classes_here
            }
        })
        .collect();
    if lanes.is_empty() {
        fail!("{CONFIG} names no step");
    }
    run_lanes(repo, commit, &lanes, &classes)
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
