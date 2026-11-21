//! `builds shots`: `builds shots findings` reads the runs builds keeps
//! (shot_findings.rs, shot_store.rs), and every other subcommand runs builds-shots, the
//! program that checks an app's shots against its design boards
//! (scripts/shots), with the same arguments. builds sets `BUILDS_SHOTS_CACHE`
//! to a folder in the build volume's caches, where the program keeps the
//! Chrome it pins and builds the design system. `builds install` puts the
//! program beside `builds`, here and on each host (install.rs).

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::tool::Tool;

/// The program's name, beside `builds` in the folder install.sh builds into
/// and in ~/.local/bin.
pub const PROGRAM: &str = "builds-shots";

/// The version of the builds-shots built from this program's sources, which
/// install puts beside it (build.rs): the version `builds-shots version`
/// prints and each run's findings.json names.
pub const VERSION: &str = env!("SHOTS_SOURCE");

const USAGE: &str = "usage: builds shots findings <repo> <ref> mac|android <prefix> [--by-cause] | builds shots run|capture|check|lint|runtime|components|states|version [<argument>...] (builds shots <command> --help)";

/// The builds-shots beside the `builds` program at <builds>.
pub fn beside(builds: &Path) -> PathBuf {
    builds.with_file_name(PROGRAM)
}

/// The builds-shots beside this program.
pub fn beside_this() -> Result<PathBuf> {
    let me = std::env::current_exe().context(|| "finding this program".to_string())?;
    Ok(beside(&me))
}

/// The cache folder builds gives the program.
fn cache(places: &Places) -> PathBuf {
    places.caches().join(PROGRAM)
}

/// Fails, with what builds-shots printed, when the boards at <tip> of the
/// repo at <top> draw a component state that no preview row shows and that
/// the listing at <base> does not hold (`builds-shots states --at <tip>
/// --against <base>`), or when builds-shots cannot tell.
pub fn require_states_shown(tool: &Tool, top: &Path, tip: &str, base: &str) -> Result<()> {
    tool.places.require_volume()?;
    let ran = Command::new(&tool.shots)
        .args(["states", "--at", tip, "--against", base])
        .current_dir(top)
        .env("BUILDS_SHOTS_CACHE", cache(&tool.places))
        .stdin(std::process::Stdio::null())
        .output()
        .context(|| format!("running {}", tool.shots.display()))?;
    if !ran.status.success() {
        fail!(
            "builds-shots refuses the change ({}):\n{}",
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
            let program = beside_this()?;
            let error = Command::new(&program)
                .args(args)
                .env("BUILDS_SHOTS_CACHE", cache(places))
                .exec();
            fail!(
                "cannot run {}: {error}. builds install puts builds-shots beside builds",
                program.display()
            )
        }
    }
}
