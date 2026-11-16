//! `builds cargo`: recording the build copies cargo used, so setup and prune
//! keep those while they delete the ones no build used for days, holding the
//! checkout's guard while the build runs, and pruning the copies it replaced
//! once it ends.
//!
//! Each run is recorded in the project's database (cargo_run.sql) when cargo
//! exits: the host it ran on with that host's load, how long cargo waited for
//! its file locks (lock_wait.rs), how long the run took, and the hook check
//! that started it. A run whose record can't be written says so on stderr and
//! ends as cargo did.

use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};

use crate::cargo_place::{self, Choice};
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::hook_stats::Load;
use crate::host::{LOCAL, Ssh};
use crate::places::{Places, Rules};
use crate::units::GUARD;
use crate::units::record::record;

/// The variable `builds checks run` sets for each check of a recorded hook
/// run (`hook_check_value`), so the `builds cargo` the check runs records
/// which check started it.
pub const HOOK_CHECK_VARIABLE: &str = "BUILDS_HOOK_CHECK";

/// The value of `HOOK_CHECK_VARIABLE` for the check at <position> of the hook
/// run that started at <started_at>: `<started_at>:<position>`.
pub fn hook_check_value(started_at: i64, position: u32) -> String {
    format!("{started_at}:{position}")
}

/// Reads a value `hook_check_value` made.
fn parse_hook_check(value: &str) -> Result<(i64, u32)> {
    let parsed = value
        .split_once(':')
        .and_then(|(at, position)| Some((at.parse().ok()?, position.parse().ok()?)));
    match parsed {
        Some(check) => Ok(check),
        None => fail!("{HOOK_CHECK_VARIABLE} is {value:?}, not <started_at>:<position>"),
    }
}

/// A `builds cargo` run that a hook check started, as the hook's figures read
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct HookCargoRun {
    pub host: String,
    pub load: Load,
    /// None when cargo printed to a terminal.
    pub lock_wait_ms: Option<u64>,
}

/// The `builds cargo` runs that the checks of the hook run that started at
/// <started_at> in <worktree> started, by the check's position; of several
/// runs one check started, the longest.
pub fn hook_runs(
    conn: &Connection,
    worktree: &str,
    started_at: i64,
    path: &Path,
) -> Result<HashMap<u32, HookCargoRun>> {
    let what = || format!("reading {}", path.display());
    let mut statement = conn
        .prepare(
            "SELECT hook_position, host, cores, load, lock_wait_ms FROM builds_cargo_runs
             WHERE worktree = ?1 AND hook_started_at = ?2 ORDER BY ms",
        )
        .context(what)?;
    let rows = statement
        .query_map(params![worktree, started_at], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                HookCargoRun {
                    host: row.get(1)?,
                    load: Load {
                        cores: row.get::<_, i64>(2)? as u32,
                        one_minute: row.get(3)?,
                    },
                    lock_wait_ms: row.get::<_, Option<i64>>(4)?.map(|ms| ms as u64),
                },
            ))
        })
        .context(what)?;
    let mut found = HashMap::new();
    // Longest last, so it is the one kept.
    for row in rows {
        let (position, run) = row.context(what)?;
        found.insert(position as u32, run);
    }
    Ok(found)
}

/// One `builds cargo` run, as it is recorded once cargo exits.
#[derive(Clone, Debug, PartialEq)]
pub struct CargoRun {
    /// Seconds since 1970.
    pub started_at: i64,
    pub repo: String,
    pub worktree: String,
    pub host: String,
    /// The host's cores and load when builds picked it.
    pub load: Load,
    /// None when cargo printed to a terminal, which builds does not read.
    pub lock_wait: Option<Duration>,
    pub took: Duration,
    /// The hook check that started it: its run's start and its position.
    pub hook_check: Option<(i64, u32)>,
}

/// Writes <run> to the database at <path>, made with its schema when it is
/// new.
pub fn record_run(path: &Path, run: &CargoRun) -> Result<()> {
    let conn = crate::db::open(path, crate::db::Make::Allowed)?;
    let ms = |took: Duration| took.as_millis() as i64;
    conn.execute(
        "INSERT INTO builds_cargo_runs (started_at, repo, worktree, host, cores, load, lock_wait_ms, ms, hook_started_at, hook_position)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            run.started_at,
            run.repo,
            run.worktree,
            run.host,
            run.load.cores,
            run.load.one_minute,
            run.lock_wait.map(ms),
            ms(run.took),
            run.hook_check.map(|(at, _)| at),
            run.hook_check.map(|(_, position)| position),
        ],
    )
    .context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Does <checkout> build with incremental compilation? Only main;
/// `role_config` says why.
pub fn builds_incrementally(checkout: &Checkout) -> bool {
    checkout.is_main()
}

/// The cargo settings a build in <checkout> takes from the checkout's role,
/// as `--config` arguments.
///
/// Every side worktree builds without incremental compilation. sccache
/// refuses every incremental compile, so with it off sccache keeps each
/// workspace crate any worktree built, and a build of the same sources in
/// another worktree, or in the app checkout, where agents take turns on
/// their own branches, comes from the cache; and the only incremental
/// folders are those a repo's lint wrapper keeps for the crates it lints,
/// which `prune` deletes after the idle hours (on 2026-11-10, before this,
/// deleting the incremental folders nothing had written for ten minutes in
/// five busy bae worktrees freed 38 GB).
/// Main, the person's checkout, keeps incremental, which halves an
/// edit-and-recheck loop's CPU in a checkout that stays on one branch.
///
/// A `--config` argument, not a `CARGO_*` variable: cargo hands its
/// environment on to rustc and sccache keys every `CARGO_*` variable, so
/// with a variable every dependency built through this command would get
/// other cache entries than the same dependency built by cargo run by hand
/// in the same checkout. The argument changes only what it sets.
fn role_config(checkout: Option<&Checkout>) -> &'static [&'static str] {
    match checkout {
        Some(checkout) if !builds_incrementally(checkout) => {
            &["--config", "build.incremental=false"]
        }
        _ => &[],
    }
}

/// Starts `builds units after-build <checkout>` in a session of its own, so
/// it outlives this process: it deletes the build copies this build
/// replaced, once nothing holds the checkout's guard or the profile's lock.
/// What it says goes to <root>/.after-build.log, which it holds an exclusive
/// flock on for its whole run (taken here, on the file description it
/// inherits as its stderr), so whoever needs it finished waits on that lock;
/// one still running from an earlier build is waited for first. Space it
/// doesn't free stays for the next prune, so a failure to start it is said
/// and the build's own result stands.
fn start_after_build(checkout: &Checkout, root: &Path) {
    use std::os::unix::io::AsRawFd;
    use std::os::unix::process::CommandExt;
    let started = (|| -> std::io::Result<()> {
        let log = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".after-build.log"))?;
        while unsafe { libc::flock(log.as_raw_fd(), libc::LOCK_EX) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        log.set_len(0)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["units", "after-build"])
            .arg(&checkout.top)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        command.spawn().map(|_| ())
    })();
    if let Err(error) = started {
        crate::text::note(format!(
            "starting the prune of the build copies this build replaced: {error}"
        ));
    }
}

/// Runs cargo with these arguments plus the flag that has it report every
/// unit it uses on stdout, which the recorder reads, and the settings of the
/// checkout's role (`role_config`). Exits with cargo's status, or the
/// recorder's when cargo succeeded and a reported unit could not be recorded.
pub fn cmd_cargo(args: &[String]) -> Result<()> {
    let (subcommand, format): (&[String], &str) = match args.first().map(String::as_str) {
        Some(
            "build" | "check" | "clippy" | "doc" | "rustc" | "rustdoc" | "test" | "bench" | "run"
            | "install",
        ) => (&args[..1], "--message-format=json-render-diagnostics"),
        Some("nextest") => match args.get(1).map(String::as_str) {
            Some("run" | "list" | "archive") => {
                (&args[..2], "--cargo-message-format=json-render-diagnostics")
            }
            other => fail!(
                "cargo: cargo nextest {} builds nothing; run it without builds",
                other.unwrap_or("")
            ),
        },
        None => fail!("usage: builds cargo <subcommand> [<args>...]"),
        Some(other) => fail!("cargo: cargo {other} builds nothing; run it without builds"),
    };
    let began = Instant::now();
    let started_at = crate::app_timing::now();
    let dir = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    let places = Places::from_env()?;
    let checkout = Checkout::at_keeping_env(&dir)?;
    let env = |name: &str| std::env::var_os(name);
    let named = std::env::var(cargo_place::HOST_VARIABLE)
        .ok()
        .filter(|name| !name.is_empty());
    let ssh = Ssh::from_env();
    let choice = match cargo_place::stays_here(args, checkout.as_ref(), &env) {
        Some(why) => {
            // A named host that cannot take this command runs it here, and
            // says so: the name is for debugging a command that could go.
            if let Some(name) = named.as_deref().filter(|name| *name != LOCAL) {
                crate::text::note(format!(
                    "{} names host {name}; this command runs on this Mac: {why}",
                    cargo_place::HOST_VARIABLE
                ));
            }
            Choice::Here(why)
        }
        None => cargo_place::choose(
            &places,
            &ssh,
            Rules::from_env()?.floor_kib,
            &checkout.as_ref().expect("a checkout was found").repo(),
            named.as_deref(),
            crate::app_timing::now(),
        )?,
    };
    let picked = match &choice {
        Choice::Here(_) => None,
        Choice::On(host, probe, _) => Some((
            host.name.clone(),
            Load {
                cores: probe.cores,
                one_minute: probe.load[0],
            },
        )),
    };
    let job = match checkout.as_ref() {
        Some(checkout) => cargo_place::start(&places, &ssh, choice, checkout, args, &env)?,
        None => None,
    };
    // A host that refused the job leaves it to this Mac.
    let (host, load) = match (&job, picked) {
        (Some(_), Some(picked)) => picked,
        _ => (LOCAL.to_string(), Load::here()),
    };
    let exited = match job {
        None => run_before_here(checkout.as_ref(), &env)
            .and_then(|()| run_here(&places, checkout.as_ref(), subcommand, format, args))?,
        Some(job) => cargo_place::finish(job)?,
    };
    if let Some(checkout) = &checkout {
        let run = CargoRun {
            started_at,
            repo: checkout.repo(),
            worktree: checkout.top.display().to_string(),
            host,
            load,
            lock_wait: exited.lock_wait,
            took: began.elapsed(),
            hook_check: None,
        };
        if let Err(error) = record_finished(&places, checkout, run) {
            crate::text::note(format!("this cargo run was not recorded: {}", error.text()));
        }
    }
    match exited.code {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

/// Records <run> in the database of <checkout>'s project, with the hook check
/// that started it, if one did; a repo in no project records nothing.
fn record_finished(places: &Places, checkout: &Checkout, mut run: CargoRun) -> Result<()> {
    let Some(database) = crate::lists::store::database_of(places, checkout)? else {
        return Ok(());
    };
    run.hook_check = match std::env::var(HOOK_CHECK_VARIABLE) {
        Ok(value) => Some(parse_hook_check(&value)?),
        Err(_) => None,
    };
    record_run(&database, &run)
}

/// How cargo ended: its exit status (or the recorder's, `run_here`), and how
/// long it waited for its file locks, when builds read its messages.
pub struct Exited {
    pub code: i32,
    pub lock_wait: Option<Duration>,
}

/// Runs the program `BUILDS_CARGO_BEFORE` names, if any, in the checkout.
fn run_before_here(
    checkout: Option<&Checkout>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<()> {
    let Some(before) = cargo_place::Before::from_env(env) else {
        return Ok(());
    };
    match checkout {
        Some(checkout) => before.run_here(checkout),
        None => fail!(
            "{} names a program to run in the checkout, and the working directory is in none",
            cargo_place::BEFORE_VARIABLE
        ),
    }
}

/// Runs cargo here, with the recorder reading its output. When this process's
/// stderr is no terminal, cargo's messages pass through builds, which times
/// cargo's waits for its file locks; to a terminal, cargo prints them itself,
/// with its colors and progress bar.
fn run_here(
    places: &Places,
    checkout: Option<&Checkout>,
    subcommand: &[String],
    format: &str,
    args: &[String],
) -> Result<Exited> {
    let config = role_config(checkout);
    let linker = crate::linker::cargo_config(&places.home);
    // Held until cargo has exited and the recorder has read its last line.
    let root = checkout
        .map(|checkout| checkout.build_root(places))
        .filter(|root| root.is_dir());
    let guard = match &root {
        Some(root) => Some(crate::evict::lock_shared_creating(&root.join(GUARD))?),
        None => None,
    };
    // SAFETY: isatty(2) reads nothing but the descriptor's kind.
    let watch = unsafe { libc::isatty(libc::STDERR_FILENO) } == 0;
    let mut child = Command::new("cargo")
        .args(subcommand)
        .arg(format)
        .args(config)
        .args(linker)
        .args(&args[subcommand.len()..])
        .env_remove(HOOK_CHECK_VARIABLE)
        .stdout(Stdio::piped())
        .stderr(if watch {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .spawn()
        .context(|| "running cargo".to_string())?;
    let relay = child
        .stderr
        .take()
        .map(|stderr| crate::lock_wait::relay(stderr, std::io::stderr()));
    let stdout = child.stdout.take().expect("piped above");
    let recorded = record(BufReader::new(stdout), &mut std::io::stdout().lock());
    let status = child.wait().context(|| "waiting for cargo".to_string())?;
    let lock_wait = relay.map(|relay| relay.join().expect("the relay does not panic"));
    drop(guard);
    if let (Some(checkout), Some(root)) = (checkout, &root) {
        start_after_build(checkout, root);
    }
    // cargo and the recorder print their own failures.
    let recorded = match recorded {
        Ok(code) => code,
        Err(Fail::Message(message)) => {
            crate::text::note(message);
            1
        }
        Err(Fail::Exit(code)) => code,
    };
    let code = if status.success() {
        recorded
    } else {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    };
    Ok(Exited { code, lock_wait })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane::Lane;
    use crate::testing::{Scratch, git, repo};

    fn role(dir: &Path) -> &'static [&'static str] {
        role_config(Checkout::at(dir).unwrap().as_ref())
    }

    #[test]
    fn only_main_builds_incrementally() {
        let scratch = Scratch::new("cargo-role");
        let places = scratch.places();
        let main = scratch.dir.join("demo");
        repo(&main, "target 1G\n");
        for name in ["side-1", "side-2"] {
            let side = scratch.dir.join("demo-worktrees").join(name);
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "--detach",
                    &side.display().to_string(),
                ],
            );
        }
        let side_1 = scratch.dir.join("demo-worktrees/side-1");
        std::fs::create_dir_all(side_1.join("crate/src")).unwrap();
        std::fs::create_dir_all(places.root.join("demo")).unwrap();
        std::fs::write(places.app_checkouts_file("demo", Lane::Mac), "side-1\n").unwrap();
        let none: &[&str] = &[];
        let off: &[&str] = &["--config", "build.incremental=false"];
        assert_eq!(role(&side_1), off, "the app checkout");
        assert_eq!(role(&side_1.join("crate/src")), off);
        assert_eq!(
            role(&scratch.dir.join("demo-worktrees/side-2")),
            off,
            "another side worktree"
        );
        assert_eq!(role(&main), none);
        assert_eq!(role(&main.join("crate")), none);
        assert_eq!(role(&scratch.dir), none, "outside any checkout");
    }
}
