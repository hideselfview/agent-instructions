//! `builds lake build [<args>...]`: runs `lake build` in the Lean package of
//! the working directory, on this Mac or on the host with the most room,
//! as `builds cargo` places a build (cargo_place.rs).
//!
//! A host gets the checkout's HEAD plus its working tree, in a worktree of its
//! own clone whose build folders, the package's `.lake` among them, live in
//! the host's builds root and stay between jobs (remote.rs). The host's setup
//! for the job installs the Lean toolchains the checkout pins (lean.rs), and
//! the host runs `builds lake` itself with `BUILDS_CARGO_HOST=local`, in the
//! folder that matches the caller's, so lake there finds the same package.
//! The caller sees lake's output and exits with its status.
//!
//! On the Mac that runs it, `builds lake` installs the checkout's Lean
//! toolchains first (unless a pre-commit hook check started it, as for
//! `builds cargo`'s prepare programs) and runs elan's `lake` while it holds
//! the checkout's guard, which keeps `builds` from deleting the checkout's
//! build folders while lake writes them.

use std::ffi::OsString;
use std::path::Path;

use crate::cargo_place::{self, Choice};
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::host::{Runs, Ssh};
use crate::places::{Places, Rules};
use crate::roots::Changed;

const USAGE: &str = "usage: builds lake build [<args>...]: builds runs lake build only; run other lake commands with lake itself";

/// `builds lake build [<args>...]`, which adds its checkout's build root to
/// <changed>.
pub fn cmd_lake(args: &[String], changed: &Changed) -> Result<()> {
    if args.first().map(String::as_str) != Some("build") {
        fail!("{USAGE}");
    }
    let dir = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    let places = Places::from_env()?;
    let checkout = Checkout::at_keeping_env(&dir)?;
    if let Some(checkout) = &checkout {
        changed.add(checkout.build_root(&places));
    }
    let env = |name: &str| std::env::var_os(name);
    let named = cargo_place::named_host(&env)?;
    let ssh = Ssh::from_env();
    let choice = match stays_here(args, checkout.as_ref()) {
        Some(why) => {
            cargo_place::note_runs_here(named.as_deref(), &why);
            Choice::Here(why)
        }
        None => cargo_place::choose(
            &places,
            &ssh,
            Rules::from_env()?.floor_kib,
            &checkout.as_ref().expect("a checkout was found").repo(),
            match named.as_deref() {
                Some(name) => cargo_place::Wanted::Named(name),
                None => cargo_place::Wanted::Room,
            },
            Runs::Lake,
            None,
            crate::app_timing::now(),
        )?,
    };
    let job = match &checkout {
        Some(checkout) => cargo_place::start(
            &places,
            &ssh,
            choice,
            checkout,
            "lake",
            host_command(args, checkout, &dir, &env),
            &env,
        )?,
        None => None,
    };
    let code = match job {
        Some(job) => cargo_place::finish_untimed(job)?,
        None => run_here(&places, checkout.as_ref(), &dir, args, &env)?,
    };
    match code {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

/// Why `builds lake <args>` must run on this Mac, or None when a host may run
/// it: a working directory in no checkout, a main checkout (the person's),
/// and an argument that is a path on this Mac stay here.
pub fn stays_here(args: &[String], checkout: Option<&Checkout>) -> Option<String> {
    if let Err(why) = cargo_place::placeable(checkout) {
        return Some(why);
    }
    if args.iter().any(|arg| arg.starts_with('/')) {
        return Some("an argument is a path on this Mac".to_string());
    }
    None
}

/// The words a host runs for `builds lake <args>` (`cargo_place::job_words`).
pub fn host_command(
    args: &[String],
    checkout: &Checkout,
    dir: &Path,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<String> {
    cargo_place::job_words("lake", "", Vec::new(), args, checkout, dir, env)
}

/// elan's `lake` on this Mac, ready to run for a checkout, with the
/// checkout's guard held until it is dropped.
pub struct Lake {
    pub command: std::process::Command,
    _guard: Option<std::fs::File>,
}

impl Lake {
    /// Installs the toolchains of <checkout>'s Lean packages, unless a
    /// pre-commit hook check started this run, takes the checkout's guard
    /// (`units::hold_guard`), and returns a command that runs elan's `lake`
    /// in <dir> with elan's home. Fails, naming `builds setup`, when this Mac
    /// has no elan.
    pub fn prepare(
        places: &Places,
        checkout: Option<&Checkout>,
        dir: &Path,
        env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Result<Lake> {
        if let Some(checkout) = checkout
            && !crate::cargo_run::in_hook(env)
        {
            crate::lean::ensure(places, &checkout.top)?;
        }
        let lake = crate::lean::lake(&places.home);
        if !lake.exists() {
            fail!(
                "this Mac has no {}: `builds setup` installs elan and the toolchains of the checkout's Lean packages",
                lake.display()
            );
        }
        let guard = crate::units::hold_guard(places, checkout)?;
        let mut command = crate::lean::elan_command(&places.home, &lake);
        command
            .current_dir(dir)
            .env_remove(crate::cargo_run::HOOK_CHECK_VARIABLE);
        Ok(Lake {
            command,
            _guard: guard,
        })
    }
}

/// Runs `lake <args>` on this Mac in <dir> (`Lake::prepare`) and returns its
/// exit status.
pub fn run_here(
    places: &Places,
    checkout: Option<&Checkout>,
    dir: &Path,
    args: &[String],
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<i32> {
    let mut lake = Lake::prepare(places, checkout, dir, env)?;
    let status = lake
        .command
        .args(args)
        .status()
        .context(|| "running lake".to_string())?;
    Ok(crate::fail::exit_code(status))
}

/// The environment variable that names a Lean executable a cargo run needs
/// (`Runner`).
pub const RUNNER_VARIABLE: &str = "BUILDS_CARGO_LEAN";

/// A Lean executable that a `builds cargo` run's tests run, such as coven's
/// narrowing differential's runner, named by
/// `BUILDS_CARGO_LEAN=<variable>=<package>:<executable>`. On whichever Mac
/// runs cargo, builds builds the executable with lake in the checkout's Lean
/// package <package> before cargo starts, and sets <variable> to its path
/// there for cargo and the tests it runs. The runner and the tests that run
/// it so always come from the same Mac and the same working tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runner {
    pub variable: String,
    /// The package's folder, relative to the checkout's top.
    pub package: String,
    pub executable: String,
}

impl Runner {
    /// The runner `RUNNER_VARIABLE` names in <env>, None when it is unset or
    /// empty; a value of another shape fails, naming the shape.
    pub fn from_env(env: &dyn Fn(&str) -> Option<OsString>) -> Result<Option<Runner>> {
        let named = crate::cargo_place::text_variable(env, RUNNER_VARIABLE)?;
        let Some(value) = named.filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let parsed = value.split_once('=').and_then(|(variable, rest)| {
            let (package, executable) = rest.rsplit_once(':')?;
            let named = |word: &str| {
                !word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            };
            let inside = !package.is_empty()
                && Path::new(package)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)));
            (named(variable) && inside && named(executable)).then(|| Runner {
                variable: variable.to_string(),
                package: package.to_string(),
                executable: executable.to_string(),
            })
        });
        match parsed {
            Some(runner) => Ok(Some(runner)),
            None => fail!(
                "{RUNNER_VARIABLE} is {value:?}, not <variable>=<package>:<executable>, with the package's folder inside the checkout"
            ),
        }
    }

    /// Builds the runner on this Mac with `lake query`, in its package in
    /// <checkout> (`Lake::prepare`), and returns its path. lake's progress
    /// goes to stderr.
    pub fn build_here(
        &self,
        places: &Places,
        checkout: &Checkout,
        env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Result<std::path::PathBuf> {
        let package = checkout.top.join(&self.package);
        let mut lake = Lake::prepare(places, Some(checkout), &package, env)?;
        let output = lake
            .command
            .args(["query", &self.executable])
            .stdin(std::process::Stdio::null())
            .stderr(crate::output::inherit())
            .output()
            .context(|| format!("running lake in {}", package.display()))?;
        if !output.status.success() {
            fail!(
                "lake could not build {} in {} ({}), so cargo did not run",
                self.executable,
                self.package,
                output.status
            );
        }
        // lake query prints the executable's path as its last line.
        let printed = String::from_utf8_lossy(&output.stdout);
        let path = printed
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| package.join(line.trim()));
        match path {
            Some(path) if path.is_file() => Ok(path),
            _ => fail!(
                "lake query {} in {} printed no path of a file: {printed:?}",
                self.executable,
                self.package
            ),
        }
    }
}

#[cfg(test)]
#[path = "lake_run_tests.rs"]
mod tests;
