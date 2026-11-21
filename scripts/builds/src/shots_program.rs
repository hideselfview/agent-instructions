//! `builds shots`: `builds shots findings` reads the cached runs
//! (shot_findings.rs, shots_cache.rs), and every other subcommand runs the
//! shots checker, which checks an app's shots against its design boards
//! (scripts/shots, a library this program is built with), with the same
//! arguments. builds gives the checker a cache folder in the build volume's
//! caches, where it keeps the Chrome it pins and builds the design system.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::tool::Tool;

/// The version of the shots checker this program is built with (build.rs):
/// the version `builds shots version` prints and each run's findings.json
/// names.
pub const VERSION: &str = env!("SHOTS_SOURCE");

const USAGE: &str = "usage: builds shots findings <repo> <ref> mac|android <prefix> [--by-cause] | builds shots run|capture|check|lint|runtime|components|states|version [<argument>...] (builds shots <command> --help)";

/// Whether `builds shots <args>` runs the checker, which uses no store:
/// every subcommand but `findings`.
pub fn runs_checker(args: &[String]) -> bool {
    args.first().is_some_and(|sub| sub != "findings")
}

/// The cache folder builds gives the checker.
fn cache(places: &Places) -> PathBuf {
    places.caches().join("builds-shots")
}

/// Fails, with what the checker printed, when the boards at <tip> of the
/// repo at <top> draw a component state that no preview row shows and that
/// the listing at <base> does not hold (`builds shots states --at <tip>
/// --against <base>`), or when the checker cannot tell.
pub fn require_states_shown(tool: &Tool, top: &Path, tip: &str, base: &str) -> Result<()> {
    let ran = Command::new(&tool.program)
        .args(["shots", "states", "--at", tip, "--against", base])
        .current_dir(top)
        .stdin(std::process::Stdio::null())
        .output()
        .context(|| format!("running {} shots states", tool.program.display()))?;
    if !ran.status.success() {
        fail!(
            "the shots checker refuses the change ({}):\n{}",
            ran.status,
            String::from_utf8_lossy(&ran.stderr).trim_end()
        );
    }
    Ok(())
}

pub fn cmd_shots(places: &Places, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        None => fail!("{USAGE}"),
        Some("findings") => crate::shot_findings::cmd_findings(places, &args[1..]),
        Some(_) => {
            places.require_volume()?;
            let checker = shots::Checker {
                version: VERSION,
                cache: &cache(places),
            };
            if shots::main(&checker, args) {
                Ok(())
            } else {
                Err(crate::fail::Fail::Exit(1))
            }
        }
    }
}
