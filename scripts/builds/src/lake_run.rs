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

const USAGE: &str = "usage: builds lake build [<args>...]: builds runs lake build only; run other lake commands with lake itself";

/// `builds lake build [<args>...]`.
pub fn cmd_lake(args: &[String]) -> Result<()> {
    if args.first().map(String::as_str) != Some("build") {
        fail!("{USAGE}");
    }
    let dir = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    let places = Places::from_env()?;
    let checkout = Checkout::at_keeping_env(&dir)?;
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

/// Runs `lake <args>` on this Mac in <dir> and returns its exit status. It
/// first installs the toolchains of <checkout>'s Lean packages, unless a
/// pre-commit hook check started it, and holds the checkout's guard while
/// lake runs.
pub fn run_here(
    places: &Places,
    checkout: Option<&Checkout>,
    dir: &Path,
    args: &[String],
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<i32> {
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
    let _guard = crate::cargo_run::guard(places, checkout)?;
    let status = crate::lean::elan_command(&places.home, &lake)
        .args(args)
        .current_dir(dir)
        .env_remove(crate::cargo_run::HOOK_CHECK_VARIABLE)
        .status()
        .context(|| format!("running {}", lake.display()))?;
    Ok(crate::fail::exit_code(status))
}

#[cfg(test)]
#[path = "lake_run_tests.rs"]
mod tests;
