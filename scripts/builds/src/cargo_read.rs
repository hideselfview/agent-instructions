//! `builds cargo` for the subcommands that only read the workspace, such as
//! `builds cargo tree` and `builds cargo metadata`.
//!
//! A read builds nothing, so it runs cargo on this Mac in the working
//! directory, with none of what `builds cargo` adds to a build (cargo_run.rs)
//! but `--locked`: no message format, no build guard or prune, no host and no
//! record of the run. cargo resolves the workspace for a read as for a build,
//! and without `--locked` it rewrites a Cargo.lock that disagrees with the
//! manifests; with it, the read fails at the cause and the checkout keeps the
//! commit's lock, as a build does.

use std::process::Command;

use crate::cargo_place;
use crate::cargo_run::locked_flag;
use crate::fail::{Context, Fail, Result};

/// cargo's subcommands that read the workspace and its lock and write
/// nothing.
pub const SUBCOMMANDS: [&str; 5] = [
    "tree",
    "metadata",
    "locate-project",
    "pkgid",
    "verify-project",
];

/// Runs `cargo <args>`, a read, on this Mac in the working directory, with
/// `--locked` (`locked_flag`), and exits with cargo's status.
pub fn run(args: &[String]) -> Result<()> {
    let env = |name: &str| std::env::var_os(name);
    cargo_place::note_runs_here(
        cargo_place::named_host(&env)?.as_deref(),
        &format!("cargo {} only reads this checkout", args[0]),
    );
    let status = Command::new("cargo")
        .arg(&args[0])
        .args(locked_flag(args))
        .args(&args[1..])
        .status()
        .context(|| "running cargo".to_string())?;
    match crate::fail::exit_code(status) {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}
