//! `builds shots`: `builds shots findings` reads the cached runs
//! (shot_findings.rs, shots_cache.rs), `builds shots picture` hands back a
//! picture of one (shots_picture.rs), `builds shots check-program` runs
//! another checkout's shots program over a cached run
//! (shots_check_program.rs), and every other subcommand runs the
//! shots checker, which checks an app's shots against its design boards
//! (scripts/shots, a library this program is built with), with the same
//! arguments. builds gives the checker a cache folder in the build volume's
//! caches, where it keeps the Chrome it pins and builds the design system.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use boards::app_build::Platform;
pub use shots::{CheckerVersions, Versions};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::tool::Tool;

/// The versions of the shots checker this program is built with (build.rs):
/// the ones `builds shots version` prints. builds caches board pictures by the
/// boards version, shots by their platform's capture version and findings by
/// the comparison version, so a change to the comparison alone makes no run
/// take its shots again, and a change to one platform's harness keeps the
/// other platforms' shots and runs (shots_cache.rs).
pub static CHECKER: LazyLock<CheckerVersions> = LazyLock::new(|| CheckerVersions {
    boards: env!("BOARDS_SOURCE").to_string(),
    mac: env!("CAPTURE_MAC_SOURCE").to_string(),
    android: env!("CAPTURE_ANDROID_SOURCE").to_string(),
    web: env!("CAPTURE_WEB_SOURCE").to_string(),
    comparison: env!("COMPARISON_SOURCE").to_string(),
});

/// The versions that make the runs of <platform>, which each run's
/// findings.json names and builds keys them on.
pub fn versions(platform: Platform) -> &'static Versions {
    static EACH: LazyLock<[Versions; 3]> =
        LazyLock::new(|| Platform::ALL.map(|platform| CHECKER.of(platform)));
    let at = Platform::ALL
        .iter()
        .position(|one| *one == platform)
        .expect("ALL lists every platform");
    &EACH[at]
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

/// The picture folder of the repo that the working directory is in, where the
/// shots checker takes board pictures from and keeps the ones it draws; none
/// outside a git checkout.
fn board_store(places: &Places) -> Result<Option<PathBuf>> {
    let dir = crate::args::current_dir()?;
    Ok(crate::checkout::Checkout::at(&dir)?
        .map(|checkout| crate::shots_cache::pictures(places, &checkout.repo())))
}

/// <args> of a `builds shots` command, with `--pictures <store>` added to
/// `draws` and `states` when <store> is known and the command names no
/// folder, so that no agent draws a page that builds' picture folder holds
/// (`builds help`).
fn with_board_store(args: &[String], store: Option<PathBuf>) -> Vec<String> {
    let mut args = args.to_vec();
    if let Some(store) = store
        && args
            .first()
            .is_some_and(|sub| sub == "draws" || sub == "states")
        && !args
            .iter()
            .any(|arg| arg == "--pictures" || arg.starts_with("--pictures="))
    {
        args.extend(["--pictures".to_string(), store.display().to_string()]);
    }
    args
}

/// `builds shots <args>` for the checker's own commands, which builds runs
/// in this process.
/// The words go to the checker as given: the checker reads them itself.
pub fn cmd_checker(places: &Places, args: &crate::args::Args) -> Result<()> {
    places.require_volume()?;
    let checker = shots::Checker {
        versions: &CHECKER,
        folders: &boards::Folders {
            cache: cache(places),
            fonts: places.fonts.clone(),
            volume: places.root.clone(),
        },
    };
    let args = with_board_store(args.words(), board_store(places)?);
    if shots::main(&checker, &args) {
        Ok(())
    } else {
        Err(crate::fail::Fail::Exit(1))
    }
}

#[cfg(test)]
#[path = "shots_program_tests.rs"]
mod tests;
