//! `builds app-build <repo> <ref> [<step>...]`: builds <ref> in the repo's app
//! checkouts with the steps its `.app-build` names. A step belongs to a lane
//! (lane.rs), and each lane has an app checkout, lock and queue of its own, so
//! the lanes the steps are in build at the same time and wait for no one but
//! their own queue.
//!
//! The command starts one `builds app-build <repo> --lane <lane> <ref>
//! [<step>...]` process per lane, each with its queue ticket (so a takeover or
//! `top` names that process) and no stdin, and prefixes every line of their
//! output with `[<lane>] `. Without a step named it runs every step, in the
//! order written; with steps named, each lane runs its own in the order
//! named. It exits 1 if either lane fails, saying which.
//!
//! A lane's process, on its lane's lock, checks <ref> out in the lane's app
//! checkout (which must be clean) and runs the lane's steps each through
//! `run_in_group`, stopping at the first one that fails. A step has a time
//! limit (app_config.rs): past it the step's process group is killed and the
//! build fails, which gives up the lock.
//! `builds app-build <repo> --list` prints the steps.
//!
//! A takeover stops the step running; the lane's build then runs again from
//! the checkout, behind the build that took over.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
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

const USAGE: &str = "usage: builds app-build <repo> <ref> [<step>...] | builds app-build <repo> --list | builds app-build <repo> --lane mac|android <ref> [<step>...]";

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

/// The names in <names> that are steps of <lane> in <config>, in order.
fn names_in(config: &Config, lane: Lane, names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| config.steps.iter().any(|step| step.name == **name && step.lane == lane))
        .cloned()
        .collect()
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
                fail!(
                    "step {} ran past its limit of {} and its process group was killed; the lock goes to the next build (a `limit {} <time>` line in {CONFIG} sets another)",
                    step.name,
                    show_duration(step.limit()),
                    step.name
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

/// The first lane of <repo> with an app checkout recorded, and its folder.
fn any_checkout(places: &Places, repo: &str) -> Result<std::path::PathBuf> {
    match Lane::ALL.into_iter().find_map(|lane| app_checkout_top(places, repo, lane).ok()) {
        Some(top) => Ok(top),
        None => fail!("{repo} has no app checkout; builds app-checkout records one"),
    }
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
/// the lane's steps to run, or all of them when none.
fn run_lane(places: &Places, repo: &str, lane: Lane, commit: &str, names: &[String]) -> Result<()> {
    let top = app_checkout_top(places, repo, lane)?;
    note(format!("waiting for {repo}'s {} app lock, then building {commit}", lane.word()));
    let mut held = lock::take(places, repo, lane)?;
    loop {
        let config = check_out(&top, commit)?;
        let steps = chosen(&config, lane, names)?;
        if steps.is_empty() {
            fail!("{CONFIG} names no step in the {} lane", lane.word());
        }
        match run_steps(&top, &steps)? {
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
fn copy_lines(from: impl Read, mut to: impl Write, prefix: String, done: std::sync::mpsc::Sender<()>) {
    let mut from = BufReader::new(from);
    let mut line = Vec::new();
    while let Ok(read) = from.read_until(b'\n', &mut line) {
        if read == 0 {
            break;
        }
        let mut out = prefix.clone().into_bytes();
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
fn run_lanes(repo: &str, commit: &str, lanes: &[(Lane, Vec<String>)]) -> Result<()> {
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
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context(|| format!("starting the {} lane's build", lane.word()))?;
        LANE_PIDS[at].store(child.id() as i32, Ordering::SeqCst);
        let (done, ended) = channel();
        let prefix = format!("[{}] ", lane.word());
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let (out_done, err_done) = (done.clone(), done);
        let out_prefix = prefix.clone();
        std::thread::spawn(move || copy_lines(stdout, std::io::stdout(), out_prefix, out_done));
        std::thread::spawn(move || copy_lines(stderr, std::io::stderr(), prefix, err_done));
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
        for step in parse_config(&text)?.steps {
            println!("{} {} {}", step.lane.word(), step.name, step.command.join(" "));
        }
        return Ok(());
    }
    if let [flag, word, commit, names @ ..] = rest
        && flag == "--lane"
        && !commit.starts_with('-')
    {
        return run_lane(places, repo, Lane::parse_or_fail(word)?, commit, names);
    }
    let [commit, names @ ..] = rest else {
        fail!("{USAGE}");
    };
    if commit.starts_with('-') {
        fail!("{USAGE}");
    }
    let config = config_at(places, repo, commit)?;
    for name in names {
        if !config.steps.iter().any(|step| step.name == *name) {
            let known: Vec<&str> = config.steps.iter().map(|step| step.name.as_str()).collect();
            fail!("{CONFIG} has no step {name} (it has: {})", known.join(", "));
        }
    }
    // The lanes the steps are in: those with a step named, or with any step
    // when none is named, each with its own names.
    let lanes: Vec<(Lane, Vec<String>)> = Lane::ALL
        .into_iter()
        .map(|lane| (lane, names_in(&config, lane, names)))
        .filter(|(lane, own)| {
            if names.is_empty() { config.steps.iter().any(|step| step.lane == *lane) } else { !own.is_empty() }
        })
        .collect();
    if lanes.is_empty() {
        fail!("{CONFIG} names no step");
    }
    run_lanes(repo, commit, &lanes)
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
