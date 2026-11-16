//! Where `builds cargo` runs cargo: on this Mac, or on the host with the most
//! room.
//!
//! The caller notices nothing: the same command prints the same output as it
//! runs, and its exit status is cargo's. A command that only compiles and
//! runs tests or lints goes to whichever reachable host has the most free
//! cores (cores minus load, less the cores of each run that started there in
//! the last minute, `Probe::room_after`; this Mac wins a tie). The host
//! builds what a build here would: the checkout's HEAD plus its working tree
//! (`What::Working`), in a worktree of its own clone with a warm `target`
//! (remote.rs). The host runs `builds cargo` itself with
//! `BUILDS_CARGO_HOST=local`, so the build there records its build copies and
//! prunes as a build here does.
//!
//! A command that must see this Mac stays here (`stays_here` says why for
//! each). A host that does not answer when asked for its load, or refuses
//! jobs (host.rs), is skipped. A host that refuses the job itself, before
//! cargo starts there (the send of the commit or the claim of a worktree
//! fails there), is recorded as refusing jobs, and the command runs on this
//! Mac. A host that stops answering while cargo runs ends the command with an
//! error naming the host: nothing runs the command again here, since cargo
//! may have run part of it there.
//!
//! `BUILDS_CARGO_HOST=<name>` picks the host for one command (`local` is this
//! Mac), for the person's debugging; a callers' own scripts never set it.

use std::ffi::OsString;

use crate::cargo_starts;
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::Result;
use crate::host::{self, Host, Ssh};
use crate::places::Places;
use crate::procs::{Place, Processes, cwd, live_args, place_of};
use crate::remote::{self, Job, Request, Start, Transport, What};
use crate::text::note;

/// The environment variable naming a program to run before cargo (`Before`).
pub const BEFORE_VARIABLE: &str = "BUILDS_CARGO_BEFORE";

/// The environment variable that names the host for one command.
pub const HOST_VARIABLE: &str = "BUILDS_CARGO_HOST";

/// What a build on a host gets from the caller's environment. Each changes
/// what the compilers or tests print or check and holds no path of this Mac.
const FORWARDED: [&str; 5] = [
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUST_BACKTRACE",
    "RUST_LOG",
    "RUST_TEST_THREADS",
];

/// An exit status of ssh itself: the connection failed or dropped. cargo and
/// the test programs it runs exit with 0, 1, 101 and the like, never 255.
const SSH_FAILED: i32 = 255;

/// Where one `builds cargo` command runs.
#[derive(Debug, PartialEq)]
pub enum Choice {
    /// This Mac, and why.
    Here(String),
    /// A host, what it said when asked for its load, and why.
    On(Host, host::Probe, String),
}

/// Why this command must run on this Mac, or None when a host may run it.
/// `env` reads the caller's environment.
///
/// - Only subcommands that compile for the checkout's own files and print
///   verdicts go: `test`, `clippy`, `check`, `doc` and `nextest run|list`.
///   `build`, `rustc` and `rustdoc` leave products in this Mac's `target`
///   that the caller then links, installs or runs (the apps' bridge
///   builds); `run` starts a program that acts on this Mac (the person's
///   app, a server, files outside the checkout); `bench` measures this
///   Mac; `install` puts programs in this Mac's `~/.cargo`.
/// - A caller that reads cargo's output (`--message-format`) reads paths on
///   this Mac, which a host's output names differently.
/// - `--target` builds for the phone or another OS with toolchains (the
///   Android NDK, the iOS SDK) installed here only.
/// - `--release` is how bae's cost tests are run, and they time themselves
///   against this Mac.
/// - A path argument (`--manifest-path`, `--target-dir`, an absolute path)
///   names a place on this Mac.
/// - A compiler wrapper given as a path, a target directory or an FFmpeg
///   folder in the environment names a program or folder on this Mac (a
///   wrapper given as a bare name, `sccache`, is the host's own).
/// - Main checkouts are the person's; builds there stay on the machine they
///   are using.
/// - A checkout of no repo, and a command a host already runs for a job
///   (`BUILDS_CARGO_HOST=local`), stay where they are.
pub fn stays_here(
    args: &[String],
    checkout: Option<&Checkout>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<String> {
    let Some(checkout) = checkout else {
        return Some("the working directory is in no git checkout".to_string());
    };
    if checkout.is_main() {
        return Some("main is the person's checkout".to_string());
    }
    let subcommand = args.first().map(String::as_str).unwrap_or("");
    let goes = match subcommand {
        "test" | "clippy" | "check" | "doc" => true,
        "nextest" => matches!(args.get(1).map(String::as_str), Some("run" | "list")),
        _ => false,
    };
    if !goes {
        return Some(format!(
            "cargo {subcommand} leaves products on this Mac, acts on it or measures it"
        ));
    }
    for arg in &args[1..] {
        if arg == "--" {
            // What follows is the programs' own arguments.
            break;
        }
        let name = arg.split('=').next().unwrap_or(arg);
        let reason = match name {
            "--message-format" | "--cargo-message-format" => {
                "the caller reads cargo's output as JSON, which names this Mac's paths"
            }
            "--target" => "a cross build needs toolchains that only this Mac has",
            "--release" => "release runs are the cost tests, which time themselves on this Mac",
            "--manifest-path" | "--target-dir" | "--config" | "--out-dir" | "--artifact-dir" => {
                "the argument names a place on this Mac"
            }
            _ if arg.starts_with('/') => "an argument is a path on this Mac",
            _ => continue,
        };
        return Some(reason.to_string());
    }
    // A compiler wrapper given as a bare program name (sccache) is found on
    // the host's own PATH; one given as a path inside the checkout is the
    // same file in the host's worktree (`host_command` rewrites the path);
    // any other path names a program on this Mac.
    for variable in WRAPPERS {
        if let Some(value) = env(variable).filter(|value| !value.is_empty())
            && value.to_string_lossy().contains('/')
            && inside(checkout, &value).is_none()
        {
            return Some(format!(
                "{variable} names a program outside the checkout, on this Mac"
            ));
        }
    }
    if let Some(before) = Before::from_env(env)
        && before.in_checkout(checkout).is_none()
    {
        return Some(format!(
            "{BEFORE_VARIABLE} names a program outside the checkout, on this Mac"
        ));
    }
    for variable in ["CARGO_TARGET_DIR", "FFMPEG_DIR"] {
        if env(variable).is_some_and(|value| !value.is_empty()) {
            return Some(format!("{variable} names a folder on this Mac"));
        }
    }
    None
}

/// The compiler wrappers cargo reads from the environment.
const WRAPPERS: [&str; 2] = ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"];

/// <path> relative to the checkout's top, when it names a file inside it.
fn inside(checkout: &Checkout, path: &OsString) -> Option<String> {
    std::path::Path::new(path)
        .strip_prefix(&checkout.top)
        .ok()
        .map(|rest| rest.to_string_lossy().into_owned())
        .filter(|rest| !rest.is_empty())
}

/// A program the caller wants run before cargo, wherever cargo runs
/// (`BUILDS_CARGO_BEFORE=<path> [<arguments>]`, split at spaces). It is for a
/// step on the checkout's own build folders that the caller cannot take on
/// this Mac alone: the host builds in folders of its own. bae's clippy script
/// uses it to make some units stale before clippy runs.
#[derive(Debug, PartialEq, Eq)]
pub struct Before {
    pub program: String,
    pub args: Vec<String>,
}

impl Before {
    /// The variable's value, None when it is unset or empty.
    pub fn from_env(env: &dyn Fn(&str) -> Option<OsString>) -> Option<Before> {
        let value = env(BEFORE_VARIABLE)?.to_string_lossy().into_owned();
        let mut words = value.split_whitespace().map(str::to_string);
        Some(Before {
            program: words.next()?,
            args: words.collect(),
        })
    }

    /// The program's path from the checkout's top, when it is a file in the
    /// checkout: a relative path that stays inside it, or an absolute path
    /// under it. The same file is in the host's worktree.
    pub fn in_checkout(&self, checkout: &Checkout) -> Option<String> {
        use std::path::{Component, Path};
        let path = Path::new(&self.program);
        let relative = if path.is_absolute() {
            path.strip_prefix(&checkout.top).ok()?
        } else {
            path
        };
        let stays = relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
        (stays && relative.components().next().is_some())
            .then(|| relative.to_string_lossy().into_owned())
    }

    /// Runs the program in the checkout's top on this Mac and fails when it
    /// does.
    pub fn run_here(&self, checkout: &Checkout) -> Result<()> {
        let program = match self.in_checkout(checkout) {
            Some(relative) => checkout.top.join(relative),
            None => std::path::PathBuf::from(&self.program),
        };
        let status = std::process::Command::new(&program)
            .args(&self.args)
            .current_dir(&checkout.top)
            .status();
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => fail!("{BEFORE_VARIABLE}: {} failed ({status})", program.display()),
            Err(error) => fail!("{BEFORE_VARIABLE}: running {}: {error}", program.display()),
        }
    }
}

/// The words a host runs for <args>: `builds cargo` with the forwarded
/// environment and `BUILDS_CARGO_HOST=local`, so the host's own `builds` runs
/// it there.
///
/// A wrapper that is a file inside <checkout> is named there by its path
/// relative to the worktree the host runs in (the host `cd`s into it first),
/// and the `Before` program, when there is one, runs there before cargo.
pub fn host_command(
    args: &[String],
    checkout: &Checkout,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<String> {
    let mut script = String::new();
    if let Some(relative) = Before::from_env(env).and_then(|before| {
        before.in_checkout(checkout).map(|relative| {
            let args: Vec<String> = before.args.iter().map(|arg| host::sh_quote(arg)).collect();
            format!("\"$PWD\"/{} {}", host::sh_quote(&relative), args.join(" "))
        })
    }) {
        script.push_str(&format!("{relative} || exit $?\n"));
    }
    for variable in WRAPPERS {
        if let Some(relative) = env(variable).and_then(|value| inside(checkout, &value)) {
            script.push_str(&format!(
                "{variable}=\"$PWD\"/{}; export {variable}\n",
                host::sh_quote(&relative)
            ));
        }
    }
    let mut words = Vec::new();
    if !script.is_empty() {
        words.extend(
            ["sh", "-c"]
                .map(String::from)
                .into_iter()
                .chain([format!("{script}exec \"$@\""), "sh".to_string()]),
        );
    }
    words.push("env".to_string());
    words.push(format!("{HOST_VARIABLE}=local"));
    for name in FORWARDED {
        if let Some(value) = env(name) {
            words.push(format!("{name}={}", value.to_string_lossy()));
        }
    }
    words.push("builds".to_string());
    words.push("cargo".to_string());
    words.extend(args.iter().cloned());
    words
}

/// Does <host> have the clone of <repo> its worktrees come from?
fn has_clone(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    repo: &str,
) -> std::result::Result<(), String> {
    let transport = Transport { places, ssh, host };
    transport
        .capture(
            &format!("test -e \"$HOME/{}/.git\"", host::clone_path(repo)),
            "looking for the repo's clone",
        )
        .map(|_| ())
        .map_err(|error| error.text())
}

/// Chooses where the command, run by this process, runs: the named host,
/// else the one with the most room that can take it, counting the runs that
/// started on each host in the last minute (cargo_starts.rs). The choice
/// joins that record at <now> before the next pick reads it. The caller has
/// already found that the command may leave this Mac (`stays_here`).
pub fn choose(
    places: &Places,
    ssh: &Ssh,
    floor_kib: u64,
    repo: &str,
    named: Option<&str>,
    now: i64,
) -> Result<Choice> {
    let starts = cargo_starts::read(places, now)?;
    let choice = pick(places, ssh, floor_kib, repo, named, &starts.recent)?;
    let host = match &choice {
        Choice::Here(_) => host::LOCAL,
        Choice::On(host, _, _) => host.name.as_str(),
    };
    starts.record(std::process::id(), host, now)?;
    Ok(choice)
}

/// Chooses the host as `choose` does, counting the recent runs in <recent>
/// against each host's room.
fn pick(
    places: &Places,
    ssh: &Ssh,
    floor_kib: u64,
    repo: &str,
    named: Option<&str>,
    recent: &cargo_starts::Recent,
) -> Result<Choice> {
    let hosts = host::all(places)?;
    if let Some(name) = named {
        let Some(found) = hosts.into_iter().find(|host| host.name == name) else {
            fail!("{HOST_VARIABLE} names host {name}, which is not recorded (`builds host list`)");
        };
        if found.target.is_none() {
            return Ok(Choice::Here(format!("{HOST_VARIABLE} names this Mac")));
        }
        let probe = match host::probe(places, ssh, &found) {
            Ok(probe) => probe,
            Err(error) => fail!("host {name} is not reachable: {error}"),
        };
        if let Err(error) = has_clone(places, ssh, &found, repo) {
            fail!("host {name} cannot build {repo}: {error}");
        }
        return Ok(Choice::On(
            found,
            probe,
            format!("named by {HOST_VARIABLE}"),
        ));
    }
    if hosts.len() == 1 {
        return Ok(Choice::Here("no other host is recorded".to_string()));
    }
    pick_ranked(
        places,
        ssh,
        repo,
        &host::probe_all(places, ssh, hosts),
        floor_kib,
        recent,
    )
}

/// The first host of <probes>, by room after the runs <recent> counts, that
/// can build <repo>; this Mac when it ranks before any that can, or when none
/// can.
fn pick_ranked(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    probes: &[(Host, std::result::Result<host::Probe, String>)],
    floor_kib: u64,
    recent: &cargo_starts::Recent,
) -> Result<Choice> {
    let (ranked, why) = host::rank_by_room(probes, floor_kib, recent);
    let mut passed_over = Vec::new();
    for (host, probe) in ranked {
        if host.target.is_none() {
            return Ok(Choice::Here(join(why, passed_over)));
        }
        match has_clone(places, ssh, &host, repo) {
            Ok(()) => return Ok(Choice::On(host, probe, join(why, passed_over))),
            Err(error) => passed_over.push(format!("{} cannot build {repo} ({error})", host.name)),
        }
    }
    Ok(Choice::Here(join(why, passed_over)))
}

fn join(why: String, more: Vec<String>) -> String {
    std::iter::once(why)
        .chain(more)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Starts the command where <choice> says. On a host, that sends
/// <checkout>'s working tree there and claims a worktree for <args>, and the
/// job comes back to be run (`finish`). A host that refuses the job (the send
/// or the claim fails there) is recorded as refusing jobs (`remote::start_job`),
/// and the command runs on this Mac instead, saying so on stderr. Returns
/// None when the command runs on this Mac.
pub fn start<'a>(
    places: &'a Places,
    ssh: &'a Ssh,
    choice: Choice,
    checkout: &Checkout,
    args: &[String],
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Option<Job<'a>>> {
    let Choice::On(host, _, why) = choice else {
        return Ok(None);
    };
    note(format!("builds cargo runs on host {}: {why}", host.name));
    // A stop signal during the send or the claim stops the job once its
    // command starts, and the job removes what it left on the host.
    remote::stop_on_signals();
    let request = Request {
        repo: checkout.repo(),
        what: What::Working,
        host: None,
        command: host_command(args, checkout, env),
    };
    match remote::start_job(places, ssh, &host, &checkout.top, &request)? {
        Start::Started(job) => Ok(Some(job)),
        Start::Refused(said) => {
            note(format!(
                "host {name} refused the job: {said}\nbuilds cargo runs it on this Mac, and passes over host {name} until a probe finds it answering with git running (`builds health` says what to do)",
                name = host.name
            ));
            Ok(None)
        }
    }
}

/// Runs the started cargo job, passing the output through, and returns
/// cargo's exit status with how long cargo waited for its file locks there. A
/// host that stops answering fails the command, naming it.
pub fn finish(job: Job) -> Result<crate::cargo_run::Exited> {
    let host = job.host_name().to_string();
    let (code, lock_wait) = job.run_timing_locks(remote::stop_on_signals())?;
    if code == SSH_FAILED {
        fail!(
            "host {host} stopped answering while cargo ran (ssh exited {SSH_FAILED}); nothing ran the command again"
        );
    }
    Ok(crate::cargo_run::Exited {
        code,
        lock_wait: Some(lock_wait),
    })
}

/// A `builds cargo` run on this Mac that uses another host: cargo runs on the
/// host, so no build process of the run runs here. The run belongs to the
/// checkout its `builds cargo` process runs in.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RemoteRun {
    /// The `builds cargo` process on this Mac.
    pub pid: i32,
    /// Index into the places it was placed among.
    #[serde(skip)]
    pub place: usize,
    /// The name of the host.
    pub host: String,
}

/// The `builds cargo` runs among <processes> that use one of <hosts>: each
/// `builds cargo` whose working directory is in one of <places> and whose
/// child is an ssh to the host, which names the host's target as one
/// argument (`Ssh::command`). `builds cargo` reaches its host through such a
/// child at every step: it asks the host for its load, sends the job, claims
/// a worktree there, runs cargo there and releases the worktree. While it
/// asks several hosts for their load before it picks one, the run uses each
/// of them. Also returns what builds could not read: the arguments of a
/// `builds` process or of a `builds cargo` child that still runs, one line
/// each, since without them builds cannot tell whether a run uses a host.
pub fn remote_runs(
    processes: &Processes,
    places: &[Place],
    hosts: &[Host],
) -> (Vec<RemoteRun>, Vec<String>) {
    let targets: Vec<(&str, &str)> = hosts
        .iter()
        .filter_map(|host| Some((host.target.as_deref()?, host.name.as_str())))
        .collect();
    let mut found = Vec::new();
    let mut unread = Vec::new();
    if targets.is_empty() {
        return (found, unread);
    }
    for process in processes.list.iter().filter(|p| p.name == "builds") {
        let argv = match live_args(process.pid) {
            Ok(Some(argv)) => argv,
            // It ended after the listing: it runs nothing now.
            Ok(None) => continue,
            Err(error) => {
                unread.push(format!(
                    "{error}, so health cannot tell whether it is a `builds cargo` that uses another host"
                ));
                continue;
            }
        };
        if argv.get(1).map(String::as_str) != Some("cargo") {
            continue;
        }
        let Some(place) = cwd(process.pid).and_then(|dir| place_of(places, &dir)) else {
            continue;
        };
        for child in processes.children(process.pid) {
            let argv = match live_args(child.pid) {
                Ok(Some(argv)) => argv,
                Ok(None) => continue,
                Err(error) => {
                    unread.push(format!(
                        "{error}; it is a child of `builds cargo` (pid {}) in {}, so health cannot tell whether that run uses another host",
                        process.pid, places[place].label
                    ));
                    continue;
                }
            };
            for (target, name) in &targets {
                if !argv.iter().skip(1).any(|arg| arg == target) {
                    continue;
                }
                let run = RemoteRun {
                    pid: process.pid,
                    place,
                    host: name.to_string(),
                };
                if !found.contains(&run) {
                    found.push(run);
                }
            }
        }
    }
    (found, unread)
}

#[cfg(test)]
#[path = "cargo_place_tests.rs"]
mod tests;
