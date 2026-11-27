//! Where `builds cargo` runs cargo: on this Mac, or on the host where it
//! finishes first.
//!
//! The caller notices nothing: the same command prints the same output as it
//! runs, and its exit status is cargo's. A command that only compiles and
//! runs tests or lints goes to the host that ran the checkout's last run of
//! the same subcommand, while that host can take it (`Warm`): each host keeps
//! a build folder of its own for the checkout, and only that host's folder
//! holds the build the run starts from. Another host's folder for the
//! checkout may be cold, and there cargo compiles again everything that
//! changed since that host's last build of it. A
//! command that no host ran for the checkout before, or whose host cannot
//! take it now, goes to whichever reachable host has the most free cores
//! (cores minus load, less the cores of each run that started there in the
//! last minute, `Probe::room_after`; this Mac wins a tie). The host
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
//! Mac. A failed install of the commit's toolchain on the host
//! (rust_toolchain.rs), which comes before the send, fails the command and
//! records no refusal. A host that stops answering while cargo runs ends the command with an
//! error naming the host: nothing runs the command again here, since cargo
//! may have run part of it there.
//!
//! `BUILDS_CARGO_HOST=<name>` picks the host for one command (`local` is this
//! Mac), for the person's debugging; a callers' own scripts never set it.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::cargo_starts;
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::host::{self, Host, Ssh};
use crate::output::note;
use crate::places::Places;
use crate::procs::{Place, Processes, live_args, place_of_process};
use crate::remote::{self, Job, Request, SSH_FAILED, Start, Transport, What};

/// The environment variable naming a program to run before cargo (`Before`).
pub const BEFORE_VARIABLE: &str = "BUILDS_CARGO_BEFORE";

/// The environment variable that names the host for one command.
pub const HOST_VARIABLE: &str = "BUILDS_CARGO_HOST";

/// The value of the variable <name>, as <env> reads the caller's
/// environment: None when it is unset. A value that is not UTF-8 fails.
pub fn text_variable(env: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Result<Option<String>> {
    env(name)
        .map(|value| match value.into_string() {
            Ok(value) => Ok(value),
            Err(value) => fail!("{name} is not UTF-8: {value:?}"),
        })
        .transpose()
}

/// The host `HOST_VARIABLE` names, as <env> reads the caller's environment:
/// None when the variable is unset or empty.
pub fn named_host(env: &dyn Fn(&str) -> Option<OsString>) -> Result<Option<String>> {
    Ok(text_variable(env, HOST_VARIABLE)?.filter(|name| !name.is_empty()))
}

/// Says on stderr that a command runs on this Mac although <named>, the host
/// `HOST_VARIABLE` names, is another host, and <why>. A named host that
/// cannot take a command runs it here and says so: the name is for debugging
/// a command that could go.
pub fn note_runs_here(named: Option<&str>, why: &str) {
    if let Some(text) = runs_here_note(named, why) {
        note(text);
    }
}

/// Runs `cargo <args>` as given on this Mac in <dir>, for a subcommand that
/// builds nothing and so gets none of what `builds cargo` adds to a build,
/// and exits with cargo's status. When <named>, the host `HOST_VARIABLE`
/// names (`named_host`), is another host, it first says the command runs
/// here, and <why>.
pub fn run_as_given(dir: &Path, args: &[String], named: Option<&str>, why: &str) -> Result<()> {
    note_runs_here(named, why);
    let status = Command::new("cargo")
        .args(args)
        .current_dir(dir)
        .status()
        .context(|| "running cargo".to_string())?;
    match crate::fail::exit_code(status) {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

/// What `note_runs_here` says, or None when <named> is no host or this Mac.
fn runs_here_note(named: Option<&str>, why: &str) -> Option<String> {
    let name = named.filter(|name| *name != host::LOCAL)?;
    Some(format!(
        "{HOST_VARIABLE} names host {name}; this command runs on this Mac: {why}"
    ))
}

/// What a build on a host gets from the caller's environment. Each changes
/// what the compilers or tests print or check and holds no path of this Mac.
/// CLIPPY_ARGS holds the lint levels of a clippy driver that runs as the
/// workspace's rustc wrapper, as coven's typed lints' driver does.
/// BUILDS_CARGO_LEAN names a Lean package by its folder in the checkout, which
/// the host's worktree holds too, and the host's `builds cargo` builds the
/// runner there (lake_run.rs).
const FORWARDED: [&str; 7] = [
    "BUILDS_CARGO_LEAN",
    "CLIPPY_ARGS",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUST_BACKTRACE",
    "RUST_LOG",
    "RUST_TEST_THREADS",
];

/// Where one `builds cargo` command runs.
#[derive(Debug, PartialEq)]
pub enum Choice {
    /// This Mac, and why.
    Here(String),
    /// A host, what it said when asked for its load, and why.
    On(Host, Box<host::Probe>, String),
}

/// Why this command must run on this Mac, or None when a host may run it.
/// `env` reads the caller's environment.
///
/// - Only subcommands that compile for the checkout's own files and print
///   verdicts go: `test`, `clippy`, `check`, `doc`, `nextest run|list` and
///   `mutants`.
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
    tool: CargoTool,
    checkout: Option<&Checkout>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<String> {
    let checkout = match placeable(checkout) {
        Ok(checkout) => checkout,
        Err(why) => return Some(why),
    };
    let subcommand = args.first().map(String::as_str).unwrap_or("");
    let goes = match tool {
        CargoTool::Nextest => matches!(args.get(1).map(String::as_str), Some("run" | "list")),
        CargoTool::Cargo => matches!(subcommand, "test" | "clippy" | "check" | "doc"),
        CargoTool::Mutants => true,
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

/// What a `builds cargo` command runs: a subcommand of cargo itself, or
/// cargo-nextest, or cargo-mutants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CargoTool {
    Cargo,
    Nextest,
    Mutants,
}

impl CargoTool {
    /// What the run of cargo subcommand <subcommand> needs of the host that
    /// takes it.
    pub fn runs(self, subcommand: &str) -> host::Runs {
        match self {
            CargoTool::Cargo if subcommand == "test" => host::Runs::Test,
            CargoTool::Cargo => host::Runs::Cargo,
            CargoTool::Nextest => host::Runs::Nextest,
            CargoTool::Mutants => host::Runs::Mutants,
        }
    }
}

/// <checkout>, when a build there may leave this Mac; else why it stays: a
/// working directory in no checkout, and a main checkout, the person's, keep
/// their builds where they are.
pub fn placeable(checkout: Option<&Checkout>) -> std::result::Result<&Checkout, String> {
    let Some(checkout) = checkout else {
        return Err("the working directory is in no git checkout".to_string());
    };
    if checkout.is_main() {
        return Err("main is the person's checkout".to_string());
    }
    Ok(checkout)
}

/// The compiler wrappers cargo reads from the environment.
const WRAPPERS: [&str; 2] = ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"];

/// <path> relative to the checkout's top, when it names a file or folder
/// inside it other than the top.
fn inside(checkout: &Checkout, path: impl AsRef<Path>) -> Option<String> {
    path.as_ref()
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
    /// does. The program's environment lacks `BEFORE_VARIABLE`, as on a host,
    /// so a `builds cargo` it runs runs no program first.
    pub fn run_here(&self, checkout: &Checkout) -> Result<()> {
        let program = match self.in_checkout(checkout) {
            Some(relative) => checkout.top.join(relative),
            None => std::path::PathBuf::from(&self.program),
        };
        let status = std::process::Command::new(&program)
            .env_remove(BEFORE_VARIABLE)
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
/// it there, in the folder of the host's worktree that matches <dir>, the
/// caller's working directory in <checkout>: cargo picks the package of the
/// folder it runs in, so a command run in a crate's folder builds that crate
/// there as it would here.
///
/// A wrapper that is a file inside <checkout> is named there by its path
/// relative to the worktree the host runs in (the host `cd`s into it first),
/// and the `Before` program, when there is one, runs there before cargo.
pub fn host_command(
    tool: CargoTool,
    args: &[String],
    checkout: &Checkout,
    dir: &Path,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<String> {
    let mut lines = String::new();
    if let Some(relative) = Before::from_env(env).and_then(|before| {
        before.in_checkout(checkout).map(|relative| {
            let args: Vec<String> = before.args.iter().map(|arg| host::sh_quote(arg)).collect();
            format!("\"$PWD\"/{} {}", host::sh_quote(&relative), args.join(" "))
        })
    }) {
        lines.push_str(&format!("{relative} || exit $?\n"));
    }
    for variable in WRAPPERS {
        if let Some(relative) = env(variable).and_then(|value| inside(checkout, &value)) {
            lines.push_str(&format!(
                "{variable}=\"$PWD\"/{}; export {variable}\n",
                host::sh_quote(&relative)
            ));
        }
    }
    let mut forwarded: Vec<String> = FORWARDED
        .iter()
        .filter_map(|name| Some(format!("{name}={}", env(name)?.to_string_lossy())))
        .collect();
    if tool == CargoTool::Mutants {
        forwarded.push(format!("{}=1", crate::cargo_mutants::CAPPED_VARIABLE));
    }
    job_words("cargo", &lines, forwarded, args, checkout, dir, env)
}

/// The words a host runs for `builds <command> <args>`, which it runs itself
/// with `BUILDS_CARGO_HOST=local` and the settings <forwarded> (`NAME=value`),
/// in the folder of its worktree that matches <dir>, the caller's working
/// directory in <checkout>. The shell lines <lines> run first, from the
/// worktree's top; each ends with a newline.
pub fn job_words(
    command: &str,
    lines: &str,
    forwarded: Vec<String>,
    args: &[String],
    checkout: &Checkout,
    dir: &Path,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<String> {
    let mut script = String::new();
    // What the host runs knows a hook check started the run, so it installs,
    // fetches and builds nothing (cargo_run::in_hook).
    if let Some(check) =
        env(crate::cargo_run::HOOK_CHECK_VARIABLE).filter(|value| !value.is_empty())
    {
        script.push_str(&format!(
            "{name}={}; export {name}\n",
            host::sh_quote(&check.to_string_lossy()),
            name = crate::cargo_run::HOOK_CHECK_VARIABLE
        ));
    }
    script.push_str(lines);
    // Last: the lines above name files from the worktree's top.
    if let Some(within) = inside(checkout, dir) {
        script.push_str(&format!("cd {} || exit $?\n", host::sh_quote(&within)));
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
    words.extend(forwarded);
    words.push("builds".to_string());
    words.push(command.to_string());
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

/// The host that ran the checkout's last run of the same subcommand, whose
/// build folder for the checkout holds the build this run starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Warm<'a> {
    pub host: &'a str,
    /// cargo's subcommand, as `builds_cargo_runs` records it (`clippy`,
    /// `nextest run`).
    pub subcommand: &'a str,
}

/// The host a command asks for before room decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wanted<'a> {
    /// `HOST_VARIABLE` names it: the command runs there, or fails when that
    /// host cannot take it.
    Named(&'a str),
    /// The command runs on the warm host when that host can take it, and
    /// on the host with the most room otherwise.
    Warm(Warm<'a>),
    /// The host with the most room.
    Room,
}

/// Chooses where the command, run by this process, runs: the host <wanted>
/// names when it can take the command, else the one with the most room that
/// can, counting the runs that started on each host in the last minute
/// (cargo_starts.rs). A job that builds <macos>, a package part of whose code
/// compiles only for macOS (cargo_needs.rs), goes only to a Mac. The choice
/// joins that record at <now> before the next pick reads it. The caller has
/// already found that the command may leave this Mac (`stays_here`).
#[allow(clippy::too_many_arguments)]
pub fn choose(
    places: &Places,
    ssh: &Ssh,
    floor_kib: u64,
    repo: &str,
    wanted: Wanted,
    runs: host::Runs,
    macos: Option<&str>,
    now: i64,
) -> Result<Choice> {
    let starts = cargo_starts::read(places, now)?;
    let choice = pick(
        places,
        ssh,
        floor_kib,
        repo,
        wanted,
        runs,
        macos,
        &starts.counts,
    )?;
    let host = match &choice {
        Choice::Here(_) => host::LOCAL,
        Choice::On(host, _, _) => host.name.as_str(),
    };
    starts.record(std::process::id(), host, now)?;
    Ok(choice)
}

/// Chooses the host as `choose` does, counting the runs <counts> holds on
/// each host (`rank`).
#[allow(clippy::too_many_arguments)]
fn pick(
    places: &Places,
    ssh: &Ssh,
    floor_kib: u64,
    repo: &str,
    wanted: Wanted,
    runs: host::Runs,
    macos: Option<&str>,
    counts: &cargo_starts::Counts,
) -> Result<Choice> {
    let hosts = host::all(places)?;
    if let Wanted::Named(name) = wanted {
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
        if let Err(why) = host::has_what_runs_needs(&found, &probe, runs) {
            fail!("{HOST_VARIABLE} names host {name}, which cannot run this: {why}");
        }
        if let Some(why) = needs_mac(&found, &probe, macos) {
            fail!("{HOST_VARIABLE} names host {name}, which cannot run this: {why}");
        }
        if let Err(error) = has_clone(places, ssh, &found, repo) {
            fail!("host {name} cannot build {repo}: {error}");
        }
        return Ok(Choice::On(
            found,
            Box::new(probe),
            format!("named by {HOST_VARIABLE}"),
        ));
    }
    if hosts.len() == 1 {
        return Ok(Choice::Here("no other host is recorded".to_string()));
    }
    let warm = match wanted {
        Wanted::Warm(warm) => Some(warm),
        Wanted::Named(_) | Wanted::Room => None,
    };
    let mut kept_off = Vec::new();
    let probes: Vec<_> = host::probe_all(places, ssh, hosts)
        .into_iter()
        .filter(|(host, probe)| {
            match probe
                .as_ref()
                .ok()
                .and_then(|probe| needs_mac(host, probe, macos))
            {
                Some(why) => {
                    kept_off.push(why);
                    false
                }
                None => true,
            }
        })
        .collect();
    // A job that keeps off a host for a reason of its own says so: the agent
    // sees why it runs where it does.
    if !kept_off.is_empty() {
        note(format!(
            "builds cargo passes over these hosts: {}",
            kept_off.join("; ")
        ));
    }
    pick_ranked(
        places,
        ssh,
        repo,
        rank(&probes, floor_kib, counts, runs, warm),
    )
}

/// Why <host>, which answered <probe>, cannot run a job that builds
/// <macos>, a package part of whose code compiles only for macOS; None when
/// it can: the job builds no such package, or the host is a Mac.
fn needs_mac(host: &Host, probe: &host::Probe, macos: Option<&str>) -> Option<String> {
    let package = macos?;
    (probe.platform.os != crate::host_platform::Os::Mac).then(|| {
        format!(
            "{} runs {}, and this job builds {package}, part of whose code compiles only for macOS (its repo's .build-dirs names it in a `macos` line)",
            host.name,
            probe.platform.os.word()
        )
    })
}

/// The hosts of <probes> that can take a run, in the order builds tries
/// them, and why. The hosts that run no more than their share of the live
/// runs <counts> holds (`over_share`) come first, and the others after them:
/// the load average never shows the compiles that wait in sccache's queue on
/// a host whose cores are all busy, so a host that runs more than its share
/// takes no new run while another host runs no more than its own. The
/// <warm> host comes first when it is within its share, and the hosts of each
/// group come by room after the recent runs <counts> holds
/// (`host::rank_by_room`).
fn rank(
    probes: &[(Host, std::result::Result<host::Probe, String>)],
    floor_kib: u64,
    counts: &cargo_starts::Counts,
    runs: host::Runs,
    warm: Option<Warm>,
) -> (Vec<(Host, host::Probe)>, String) {
    let (ranked, mut why) = host::rank_by_room(probes, floor_kib, &counts.recent, runs);
    // The live runs and this one split by cores always leave at least one
    // host within its share: `over_share`.
    let over = over_share(&ranked, &counts.live);
    let (mut ranked, beyond): (Vec<_>, Vec<_>) = ranked
        .into_iter()
        .partition(|(host, _)| !over.iter().any(|over| over.host == host.name));
    let first_group = ranked.len();
    ranked.extend(beyond);
    for over in &over {
        why.push_str(&format!(
            "; {} runs {}, more than its share of {} by cores, and takes no new run while another host runs no more than its share",
            over.host,
            crate::text::counted(over.live as usize, "live cargo run", "live cargo runs"),
            over.share
        ));
    }
    // The warm host goes first unless it runs more than its share.
    let Some((warm, at)) = warm.and_then(|warm| {
        ranked[..first_group]
            .iter()
            .position(|(host, _)| host.name == warm.host)
            .map(|at| (warm, at))
    }) else {
        return (ranked, why);
    };
    let first = ranked.remove(at);
    ranked.insert(0, first);
    let why = format!(
        "{} ran this checkout's last cargo {}, and its build folder holds that build; {why}",
        warm.host, warm.subcommand
    );
    (ranked, why)
}

/// A host that runs more than its share of the live runs.
#[derive(Debug, PartialEq, Eq)]
struct OverShare {
    host: String,
    /// The live runs builds placed there that still run.
    live: u32,
    /// Its share of all the live runs and this one, by its cores against
    /// the cores of all of <ranked>, rounded up.
    share: u32,
}

/// The hosts of <ranked> whose live runs (<live>) and this run would pass
/// their share: all the live runs on <ranked> and this one, split by cores,
/// rounded up. The shares add up to at least all those runs, so at least
/// one host stays within its share.
fn over_share(ranked: &[(Host, host::Probe)], live: &cargo_starts::Recent) -> Vec<OverShare> {
    let runs_on = |host: &Host| live.get(&host.name).copied().unwrap_or(0);
    let all = ranked.iter().map(|(host, _)| runs_on(host)).sum::<u32>() + 1;
    let cores: u32 = ranked.iter().map(|(_, probe)| probe.cores).sum();
    if cores == 0 {
        return Vec::new();
    }
    ranked
        .iter()
        .filter_map(|(host, probe)| {
            let share = (u64::from(all) * u64::from(probe.cores)).div_ceil(u64::from(cores)) as u32;
            let live = runs_on(host);
            (live + 1 > share).then(|| OverShare {
                host: host.name.clone(),
                live,
                share,
            })
        })
        .collect()
}

/// The first host of <ranked> (`rank`) that can build <repo>; this Mac when
/// it comes before any that can, or when none can.
fn pick_ranked(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    (ranked, why): (Vec<(Host, host::Probe)>, String),
) -> Result<Choice> {
    let mut passed_over = Vec::new();
    for (host, probe) in ranked {
        if host.target.is_none() {
            return Ok(Choice::Here(join(why, passed_over)));
        }
        match has_clone(places, ssh, &host, repo) {
            Ok(()) => return Ok(Choice::On(host, Box::new(probe), join(why, passed_over))),
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

/// Starts `builds <command>` where <choice> says. On a host, that sends
/// <checkout>'s working tree there and claims a worktree, where the job runs
/// <words> (`host_command`, `job_words`), and the job comes back to be run
/// (`finish`, `finish_untimed`). A host that refuses the job (the send or the
/// claim fails there) is recorded as refusing jobs (`remote::start_job`), and
/// the command runs on this Mac instead, saying so on stderr. Returns None
/// when the command runs on this Mac.
pub fn start<'a>(
    places: &'a Places,
    ssh: &'a Ssh,
    choice: Choice,
    checkout: &Checkout,
    command: &str,
    words: Vec<String>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Option<Job<'a>>> {
    let Choice::On(host, _, why) = choice else {
        return Ok(None);
    };
    note(format!(
        "builds {command} runs on host {}: {why}",
        host.name
    ));
    // A stop signal during the send or the claim stops the job once its
    // command starts, and the job removes what it left on the host.
    remote::stop_on_signals();
    // The host's setup for a hook's job prepares nothing.
    let request = Request {
        repo: checkout.repo(),
        prepare: if crate::cargo_run::in_hook(env) {
            crate::setup::Prepare::No
        } else {
            crate::setup::Prepare::Yes
        },
        what: What::Working,
        host: None,
        command: words,
    };
    match remote::start_job(places, ssh, &host, &checkout.top, &request)? {
        Start::Started(job) => Ok(Some(*job)),
        Start::Refused(said) => {
            note(format!(
                "host {name} refused the job: {said}\nbuilds {command} runs it on this Mac, and passes over host {name} until a probe finds it answering with git running (`builds health` says what to do)",
                name = host.name
            ));
            Ok(None)
        }
    }
}

/// Runs the started cargo job, passing the output through and giving each
/// line to <watch>, and returns cargo's exit status with how long cargo
/// waited for its file locks there. A host that stops answering fails the
/// command, naming it.
pub fn finish(
    job: Job,
    watch: &crate::cargo_run_outcome::Watch,
) -> Result<crate::cargo_run::Exited> {
    let host = job.host_name().to_string();
    let (code, lock_wait) = job.run_timing_locks(watch)?;
    answered(&host, code)?;
    Ok(crate::cargo_run::Exited {
        code,
        lock_wait: Some(lock_wait),
    })
}

/// Runs a started job whose command builds does not time, passing the output
/// through, and returns its exit status. A host that stops answering fails
/// the command, naming it.
pub fn finish_untimed(job: Job) -> Result<i32> {
    let host = job.host_name().to_string();
    let code = job.run()?;
    answered(&host, code)?;
    Ok(code)
}

/// Fails, naming <host>, when <code> is ssh's own failure: the connection to
/// the host failed while the job ran. cargo and the test programs it runs
/// exit with 0, 1, 101 and the like, never 255.
fn answered(host: &str, code: i32) -> Result<()> {
    if code == SSH_FAILED {
        fail!(
            "the connection to host {host} failed while the command ran (ssh exited {SSH_FAILED}); nothing ran the command again"
        );
    }
    Ok(())
}

/// A `builds cargo` or `builds lake` run on this Mac that uses another host:
/// cargo or lake runs on the host, so no build process of the run runs here.
/// The run belongs to the checkout its `builds` process runs in.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RemoteRun {
    /// The `builds` process on this Mac.
    pub pid: i32,
    /// The index in <places> of the place that holds the working directory
    /// of the `builds` process.
    #[serde(skip)]
    pub place: usize,
    /// The name of the host.
    pub host: String,
    /// What the run runs there: `cargo` or `lake`.
    pub command: String,
}

/// The `builds` commands that run their build on a host (`start`).
const PLACED: [&str; 2] = ["cargo", "lake"];

/// The `builds cargo` and `builds lake` runs among <processes> that use one
/// of <hosts>: each such `builds` whose working directory is in one of
/// <places> and whose child is an ssh to the host, which names the host's
/// target as one argument (`Ssh::command`). Such a `builds` reaches its host
/// through such a child at every step: it asks the host for its load, sends
/// the job, claims a worktree there, runs its build there and releases the
/// worktree. While it asks several hosts for their load before it picks one,
/// the run uses each of them. Also returns what builds could not read: the
/// arguments of a `builds` process or of a child of such a run that still
/// runs, one line each, since without them builds cannot tell whether a run
/// uses a host.
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
                    "{error}, so health cannot tell whether it is a `builds cargo` or `builds lake` that uses another host"
                ));
                continue;
            }
        };
        let Some(command) = argv
            .get(1)
            .filter(|command| PLACED.contains(&command.as_str()))
        else {
            continue;
        };
        let Some(place) = place_of_process(places, process.pid) else {
            continue;
        };
        for child in processes.children(process.pid) {
            let argv = match live_args(child.pid) {
                Ok(Some(argv)) => argv,
                Ok(None) => continue,
                Err(error) => {
                    unread.push(format!(
                        "{error}; it is a child of `builds {command}` (pid {}) in {}, so health cannot tell whether that run uses another host",
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
                    command: command.clone(),
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
