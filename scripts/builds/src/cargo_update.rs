//! `builds cargo update`: cargo's update of the Cargo.lock entries of named
//! packages, such as `builds cargo update -p coven` after a pin's `rev`
//! changes in Cargo.toml.
//!
//! An update builds nothing: it rewrites Cargo.lock and fetches what the new
//! entries name into cargo's own caches. So it runs cargo as given, on this
//! Mac in the working directory, with none of what `builds cargo` adds to a
//! build (cargo_run.rs): no `--locked`, which would refuse the rewrite, no
//! message format, which `cargo update` does not take, no build guard or
//! prune, no host and no record of the run.
//!
//! An update that names no package moves every dependency in the lock to its
//! newest compatible version, which no pin move needs, so builds refuses it.

use std::path::Path;

use crate::fail;
use crate::fail::{Context, Result};

/// The options of `cargo update` that take the next argument as their value
/// when it is not joined with `=`, so that argument names no package.
const VALUED: [&str; 6] = [
    "--precise",
    "--color",
    "--config",
    "--manifest-path",
    "--lockfile-path",
    "-Z",
];

/// Why builds refuses `cargo <args>`, an update, or None when it names a
/// package (`-p`, `--package` or a bare spec) or the workspace (`-w`,
/// `--workspace`), which limits the update to those entries.
pub fn refusal(args: &[String]) -> Option<String> {
    let mut words = args.iter().skip(1);
    while let Some(word) = words.next() {
        let named = match word.as_str() {
            "--" => words.next().is_some(),
            "-w" | "--workspace" => true,
            "-p" | "--package" => words.next().is_some(),
            option if VALUED.contains(&option) => {
                words.next();
                false
            }
            option if option.starts_with('-') => {
                option.starts_with("-p") || option.starts_with("--package=")
            }
            _ => true,
        };
        if named {
            return None;
        }
    }
    Some(
        "cargo update names no package, so it would move every dependency in Cargo.lock to its newest compatible version; name each package to update (`builds cargo update -p <package>`)"
            .to_string(),
    )
}

/// Runs `cargo <args>`, an update, in the working directory, unless
/// `refusal` refuses it, and exits with cargo's status.
pub fn run(args: &[String]) -> Result<()> {
    let dir = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    run_in(&dir, args)
}

/// Runs `cargo <args>`, an update, in <dir>, as `run` does.
fn run_in(dir: &Path, args: &[String]) -> Result<()> {
    if let Some(why) = refusal(args) {
        fail!("{why}");
    }
    crate::cargo_place::run_as_given(dir, args, "cargo update changes this checkout's Cargo.lock")
}

#[cfg(test)]
#[path = "cargo_update_tests.rs"]
mod tests;
