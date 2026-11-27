//! The Rust toolchain a checkout pins (`rust-toolchain.toml` or
//! `rust-toolchain`), and installing it on a machine before cargo runs
//! there.
//!
//! builds turns rustup's own install off for itself and everything it starts
//! (`AUTO_INSTALL_VARIABLE`), because rustup installs under no lock: two
//! cargos that install one toolchain at once can leave it listing a
//! component whose program is missing. builds installs it instead
//! (`ensure`), on each machine that runs cargo, and on a job's host as a
//! step of its own before the job (`ensure_on`).
//!
//! Each machine keeps three files per channel in `<builds root>/.toolchains`:
//!
//! - `<channel>.lock`: a check of the toolchain holds it shared and
//!   `rustup toolchain install` holds it exclusive, so a check never reads a
//!   toolchain that an install is writing.
//! - `<channel>.prepare.lock`: whatever adds to the toolchain holds it:
//!   builds' install, and the repo's `prepare` programs and tool builds,
//!   which add components of their own (coven's add rustc-dev). A check never
//!   takes it, so a long prepare program holds up no job whose toolchain is
//!   complete.
//! - `<channel>.last`: the last install's request and how it ended. A job
//!   that waited while an install of the same request failed fails with its
//!   error; a job that arrives later installs again.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::flock::{lock_creating, try_lock_creating};
use crate::host::sh_quote;
use crate::output::note;
use crate::places::Places;
use crate::remote::{SSH_FAILED, Transport, root_script};
use crate::system::{Program, System};

/// rustup's setting that, at `0`, keeps it from installing a toolchain that
/// a command needs.
pub const AUTO_INSTALL_VARIABLE: &str = "RUSTUP_AUTO_INSTALL";

/// The variable that tells a `builds` started under a held prepare lock which
/// channel's lock its caller holds, so it does not wait for it.
pub const PREPARING_VARIABLE: &str = "BUILDS_TOOLCHAIN_PREPARING";

/// The toolchain files rustup reads in a folder, the one that wins first.
pub const FILES: [&str; 2] = ["rust-toolchain", "rust-toolchain.toml"];

/// The folder in the builds root that holds each channel's locks and record.
const FOLDER: &str = ".toolchains";

/// The components that have a program, each with the program's name.
const PROGRAMS: [(&str, &str); 4] = [
    ("rustc", "rustc"),
    ("cargo", "cargo"),
    ("rustfmt", "rustfmt"),
    ("clippy", "cargo-clippy"),
];

/// What a toolchain file asks rustup for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toolchain {
    pub channel: String,
    pub profile: Option<String>,
    pub components: Vec<String>,
    pub targets: Vec<String>,
}

/// Reads <text>, which toolchain file <file> holds, as rustup does. None when
/// it names no channel, such as a file that names a toolchain by its path.
pub fn parse(file: &str, text: &str) -> std::result::Result<Option<Toolchain>, String> {
    // The older `rust-toolchain` file may hold only the channel's name.
    let legacy =
        file == "rust-toolchain" && !text.contains(['[', '=']) && text.trim().lines().count() <= 1;
    let toolchain = if legacy {
        Some(Toolchain {
            channel: text.trim().to_string(),
            profile: None,
            components: Vec::new(),
            targets: Vec::new(),
        })
    } else {
        let table = text
            .parse::<toml::Table>()
            .map_err(|error| format!("is not TOML: {error}"))?;
        let section = table.get("toolchain");
        let field = |name: &str| section.and_then(|section| section.get(name));
        let word = |name: &str| field(name).and_then(toml::Value::as_str);
        let words = |name: &str| -> std::result::Result<Vec<String>, String> {
            let Some(value) = field(name) else {
                return Ok(Vec::new());
            };
            value
                .as_array()
                .and_then(|list| {
                    list.iter()
                        .map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .ok_or_else(|| format!("{name} is not a list of names"))
        };
        match word("channel") {
            Some(channel) => Some(Toolchain {
                channel: channel.to_string(),
                profile: word("profile").map(str::to_string),
                components: words("components")?,
                targets: words("targets")?,
            }),
            None => None,
        }
    };
    match toolchain {
        // The channel names builds' files for it.
        Some(toolchain) if !crate::built_tools::is_plain_name(&toolchain.channel) => Err(format!(
            "channel {:?} is no toolchain's name",
            toolchain.channel
        )),
        toolchain => Ok(toolchain),
    }
}

fn parse_file(file: &str, text: &str) -> Result<Option<Toolchain>> {
    parse(file, text).map_err(|error| Fail::Message(format!("{file}: {error}")))
}

/// The toolchain rustup picks for a command in <dir> of the checkout at
/// <top>: the one the nearest toolchain file from <dir> up to <top> names.
/// None when no folder has a file.
pub fn in_checkout(top: &Path, dir: &Path) -> Result<Option<Toolchain>> {
    let within = dir
        .ancestors()
        .take_while(|folder| folder.starts_with(top))
        .chain((!dir.starts_with(top)).then_some(top));
    for folder in within {
        for file in FILES {
            let path = folder.join(file);
            match std::fs::read_to_string(&path) {
                Ok(text) => return parse_file(file, &text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => fail!("reading {}: {error}", path.display()),
            }
        }
    }
    Ok(None)
}

/// The toolchain file at the top of <revision> in the repo at <repo>, with
/// its name; None when the commit has none.
pub fn file_at(repo: &Path, revision: &str) -> Result<Option<(&'static str, String)>> {
    for file in FILES {
        let listed = crate::app_config::run_git(repo, &["ls-tree", "--name-only", revision, file])?;
        if !listed.trim().is_empty() {
            let text = crate::app_config::run_git(repo, &["show", &format!("{revision}:{file}")])?;
            return Ok(Some((file, text)));
        }
    }
    Ok(None)
}

/// The toolchain that <revision> of the repo at <repo> pins at its top.
pub fn at_commit(repo: &Path, revision: &str) -> Result<Option<Toolchain>> {
    match file_at(repo, revision)? {
        Some((file, text)) => parse_file(file, &text),
        None => Ok(None),
    }
}

/// `ensure` for the toolchain rustup picks in <dir> of the checkout at <top>.
pub fn ensure_in(places: &Places, system: &System, top: &Path, dir: &Path) -> Result<()> {
    match in_checkout(top, dir)? {
        Some(toolchain) => ensure(places, system, &toolchain),
        None => Ok(()),
    }
}

/// How one install ended.
#[derive(Serialize, Deserialize)]
struct Attempt {
    /// A name no other attempt has.
    id: String,
    request: Toolchain,
    failed: Option<String>,
}

fn folder(places: &Places) -> Result<PathBuf> {
    let folder = places.root.join(FOLDER);
    std::fs::create_dir_all(&folder).context(|| format!("creating {}", folder.display()))?;
    Ok(folder)
}

fn last_attempt(path: &Path) -> Result<Option<Attempt>> {
    let what = || format!("reading {}", path.display());
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).context(what),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// Writes <attempt> whole, through a file it then renames.
fn record_attempt(path: &Path, attempt: &Attempt) -> Result<()> {
    let what = || format!("writing {}", path.display());
    let staged = path.with_extension(format!("last.{}", crate::text::staged_suffix()));
    std::fs::write(&staged, serde_json::to_vec(attempt).context(what)?).context(what)?;
    std::fs::rename(&staged, path).context(what)
}

/// Takes <how> on the lock at <path>. When another process holds it, first
/// tells <say> that builds waits while another job <what>.
fn wait_for(path: &Path, how: libc::c_int, what: &str, say: &dyn Fn(String)) -> Result<File> {
    if let Some(lock) = try_lock_creating(path, how)? {
        return Ok(lock);
    }
    say(format!("builds waits while another builds job {what}"));
    lock_creating(path, how)
}

/// The hold on a channel's prepare lock, by this process or by the `builds`
/// that started it.
pub struct Preparing {
    channel: Option<String>,
    lock: Option<File>,
}

impl Preparing {
    /// Tells <command>, which runs under the hold, that its channel is held.
    pub fn pass_to(&self, command: &mut Command) {
        if let Some(channel) = &self.channel {
            command.env(PREPARING_VARIABLE, channel);
        }
    }
}

/// Takes the prepare lock of <toolchain>'s channel on this machine, unless
/// the `builds` that started this one holds it (`Places::preparing`). A
/// checkout that pins no toolchain takes none.
pub fn preparing(places: &Places, toolchain: Option<&Toolchain>) -> Result<Preparing> {
    preparing_saying(places, toolchain, &note_wait)
}

fn note_wait(words: String) {
    note(words);
}

fn preparing_saying(
    places: &Places,
    toolchain: Option<&Toolchain>,
    say: &dyn Fn(String),
) -> Result<Preparing> {
    let Some(toolchain) = toolchain else {
        return Ok(Preparing {
            channel: None,
            lock: None,
        });
    };
    let channel = &toolchain.channel;
    let lock = match places.preparing.as_deref() == Some(channel.as_str()) {
        true => None,
        false => Some(wait_for(
            &folder(places)?.join(format!("{channel}.prepare.lock")),
            libc::LOCK_EX,
            &format!("adds to Rust toolchain {channel}"),
            say,
        )?),
    };
    Ok(Preparing {
        channel: Some(channel.clone()),
        lock,
    })
}

/// Makes sure this machine has <toolchain> with its components, its targets
/// and their programs, installing what it lacks. One install of a channel
/// runs at a time; a job that waited for an install of the same request that
/// failed fails with its error.
pub fn ensure(places: &Places, system: &System, toolchain: &Toolchain) -> Result<()> {
    ensure_saying(places, system, toolchain, &note_wait)
}

/// `ensure`, telling <say> of each wait for another job.
fn ensure_saying(
    places: &Places,
    system: &System,
    toolchain: &Toolchain,
    say: &dyn Fn(String),
) -> Result<()> {
    let folder = folder(places)?;
    let channel = &toolchain.channel;
    let lock = folder.join(format!("{channel}.lock"));
    let record = folder.join(format!("{channel}.last"));
    // Read before any wait: an attempt recorded after this one ran while this
    // job waited.
    let seen = last_attempt(&record)?.map(|attempt| attempt.id);
    let installs = format!("installs Rust toolchain {channel}");
    {
        let _no_install = wait_for(&lock, libc::LOCK_SH, &installs, say)?;
        if lacking(system, toolchain)?.is_none() {
            return Ok(());
        }
    }
    let preparing = preparing_saying(places, Some(toolchain), say)?;
    let installing = wait_for(&lock, libc::LOCK_EX, &installs, say)?;
    if let Some(attempt) = last_attempt(&record)?
        && Some(&attempt.id) != seen.as_ref()
        && attempt.request == *toolchain
        && let Some(error) = attempt.failed
    {
        return Err(Fail::Message(error));
    }
    let Some(why) = lacking(system, toolchain)? else {
        return Ok(());
    };
    note(format!(
        "builds installs Rust toolchain {channel}, since this machine's is incomplete: {why}"
    ));
    let held: Vec<&File> = preparing.lock.iter().chain([&installing]).collect();
    let installed = install(system, toolchain, &held);
    let recorded = record_attempt(
        &record,
        &Attempt {
            id: format!("{}-{}", crate::job_claim::now_nanos(), std::process::id()),
            request: toolchain.clone(),
            failed: installed.as_ref().err().map(ToString::to_string),
        },
    );
    crate::fail::with_cleanup(installed, recorded)?;
    note(format!("builds installed Rust toolchain {channel}"));
    Ok(())
}

/// Runs rustup with <args> to its end.
fn rustup(system: &System, args: &[&str]) -> Result<Command> {
    let mut command = system.command(Program::Rustup)?;
    command.args(args).stdin(Stdio::null());
    Ok(command)
}

fn output(mut command: Command) -> Result<Output> {
    command.output().context(|| "running rustup".to_string())
}

/// What rustup said when it failed.
fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// What of <toolchain> this machine lacks, or None when it has all of it:
/// the toolchain, each component and target, rustc and cargo, and the program
/// of each component that has one. rustup can list a component whose program
/// is missing, so each program is asked for too.
fn lacking(system: &System, toolchain: &Toolchain) -> Result<Option<String>> {
    let channel = toolchain.channel.as_str();
    // The lines rustup printed, or what it said when it failed.
    let ask = |args: &[&str]| -> Result<std::result::Result<Vec<String>, String>> {
        let output = output(rustup(system, args)?)?;
        Ok(match output.status.success() {
            true => Ok(String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| line.trim().to_string())
                .collect()),
            false => Err(said(&output)),
        })
    };
    let installed = match ask(&["component", "list", "--installed", "--toolchain", channel])? {
        Ok(installed) => installed,
        Err(said) => return Ok(Some(said)),
    };
    let version = match ask(&["run", channel, "rustc", "-vV"])? {
        Ok(version) => version,
        Err(said) => return Ok(Some(said)),
    };
    let Some(host) = version.iter().find_map(|line| line.strip_prefix("host: ")) else {
        fail!("`rustup run {channel} rustc -vV` names no host: {version:?}");
    };
    // A toolchain file may name a component by its name before it was stable
    // (`llvm-tools-preview`), which rustup lists without the suffix.
    let mut required: Vec<&str> = vec!["rustc", "cargo"];
    required.extend(
        toolchain
            .components
            .iter()
            .map(|name| name.strip_suffix("-preview").unwrap_or(name)),
    );
    for name in &required {
        let for_host = format!("{name}-{host}");
        if !installed
            .iter()
            .any(|line| line == name || *line == for_host)
        {
            return Ok(Some(format!("it lacks the component {name}")));
        }
    }
    if !toolchain.targets.is_empty() {
        let installed = match ask(&["target", "list", "--installed", "--toolchain", channel])? {
            Ok(installed) => installed,
            Err(said) => return Ok(Some(said)),
        };
        if let Some(target) = toolchain
            .targets
            .iter()
            .find(|target| !installed.contains(target))
        {
            return Ok(Some(format!("it lacks the target {target}")));
        }
    }
    for (component, program) in PROGRAMS {
        if required.contains(&component)
            && let Err(said) = ask(&["which", "--toolchain", channel, program])?
        {
            return Ok(Some(said));
        }
    }
    Ok(None)
}

/// Installs what <toolchain> asks for, and fails unless the machine then has
/// all of it. rustup holds the <locks> too, so they last while it runs, even
/// when this process is killed first.
fn install(system: &System, toolchain: &Toolchain, locks: &[&File]) -> Result<()> {
    let channel = toolchain.channel.as_str();
    let mut args = vec!["toolchain", "install", channel, "--no-self-update"];
    if let Some(profile) = &toolchain.profile {
        args.extend(["--profile", profile]);
    }
    let components = toolchain.components.join(",");
    if !components.is_empty() {
        args.extend(["--component", &components]);
    }
    let targets = toolchain.targets.join(",");
    if !targets.is_empty() {
        args.extend(["--target", &targets]);
    }
    let mut command = rustup(system, &args)?;
    let descriptors: Vec<_> = locks.iter().map(|lock| lock.as_raw_fd()).collect();
    // SAFETY: between fork and exec the closure only calls fcntl.
    unsafe {
        command.pre_exec(move || {
            descriptors
                .iter()
                .try_for_each(|fd| crate::flock::inherit(*fd))
        });
    }
    let ran = output(command)?;
    if !ran.status.success() {
        fail!(
            "`rustup {}` failed ({}): {}",
            args.join(" "),
            ran.status,
            said(&ran)
        );
    }
    // rustup calls a toolchain that lists a component unchanged, even when
    // the component's program is missing: only an uninstall repairs that.
    match lacking(system, toolchain)? {
        None => Ok(()),
        Some(why) => fail!(
            "`rustup {}` succeeded, and the toolchain is still incomplete: {why}\nRun `rustup toolchain uninstall {channel}` on this machine, then run the command again",
            args.join(" ")
        ),
    }
}

/// How `ensure_on` went on a host.
#[derive(Debug, PartialEq, Eq)]
pub enum OnHost {
    Ready,
    /// ssh did not reach the host, or lost it: what it said.
    Unreachable(String),
}

/// `ensure` on the host of <transport>, which runs its own `builds toolchain`.
/// A failed install is an error that names the host and holds what its builds
/// said.
pub fn ensure_on(transport: &Transport, toolchain: &Toolchain) -> Result<OnHost> {
    if transport.host.target.is_none() {
        ensure(transport.places, transport.ssh.system(), toolchain)?;
        return Ok(OnHost::Ready);
    }
    let mut words = format!("toolchain {}", sh_quote(&toolchain.channel));
    if let Some(profile) = &toolchain.profile {
        words.push_str(&format!(" --profile {}", sh_quote(profile)));
    }
    for (flag, names) in [
        ("--component", &toolchain.components),
        ("--target", &toolchain.targets),
    ] {
        for name in names {
            words.push_str(&format!(" {flag} {}", sh_quote(name)));
        }
    }
    let script = format!(
        "{}\"$HOME/.local/bin/builds\" {words}",
        root_script(transport.host)
    );
    let name = &transport.host.name;
    let ran = transport
        .command(&script)?
        .stdin(Stdio::null())
        .output()
        .context(|| format!("installing a Rust toolchain on host {name}"))?;
    let printed = said(&ran);
    if ran.status.success() {
        if !printed.is_empty() {
            note(format!("host {name}: {printed}"));
        }
        return Ok(OnHost::Ready);
    }
    let failure = format!(
        "installing Rust toolchain {} failed on host {name} ({}): {printed}",
        toolchain.channel, ran.status
    );
    match ran.status.code() {
        Some(SSH_FAILED) => Ok(OnHost::Unreachable(failure)),
        _ => Err(Fail::Message(failure)),
    }
}

/// `builds toolchain <channel>`: `ensure` on this machine.
pub fn cmd_toolchain(places: &Places, system: &System, args: &crate::args::Args) -> Result<()> {
    let [channel] = args.positional() else {
        fail!("{}", crate::command::Command::Toolchain.usage_text());
    };
    let names = |flag: &str| {
        args.values(flag)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    ensure(
        places,
        system,
        &Toolchain {
            channel: channel.clone(),
            profile: args.value("profile")?.map(str::to_string),
            components: names("component"),
            targets: names("target"),
        },
    )
}

#[cfg(test)]
#[path = "rust_toolchain_tests.rs"]
mod tests;
