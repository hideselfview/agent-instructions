//! The record of an app build lane's run when an install moved the stores
//! while the lane built.
//!
//! An install never stops an app build: each lane builds to its end on the
//! programs it started with (programs.rs) and writes its run when it ends
//! (app_timing.rs). An install that changed a store's schema meanwhile moved
//! the store to the installed program's schema, which this program cannot
//! write. So the lane writes its run to a new store of its own schema, in a
//! folder of its own under the install folder, and runs the installed builds
//! (`builds app-build --record-handed <repo> <store>`), which moves that
//! store to its schema with the moves it makes for every store (migrate.rs),
//! reads each run back and records it in the store of the repo's project.
//! Then the lane deletes the folder. The run reaches the store whole or the
//! record fails, as any record does.

use std::path::{Path, PathBuf};

use crate::app_timing::{self, DATABASE, RunRecord};
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;

/// The `builds app-build` flag of the form that records a handed store.
pub const FLAG: &str = "--record-handed";

/// The folder of the stores that lanes hand the installed builds.
pub fn folder(places: &Places) -> PathBuf {
    places.install_dir().join("handed-runs")
}

/// Writes <run> to the store at <database>. When the store has another
/// schema and <install_ran> says an install that moved the stores ran since
/// this process started, writes the run to a new store of this schema in a
/// new folder in <folder> and hands that store to <hand_off>, then deletes
/// the folder, whether or not the hand-off succeeded.
pub fn write(
    database: &Path,
    run: &RunRecord,
    folder: &Path,
    install_ran: &dyn Fn() -> Result<bool>,
    hand_off: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let refused = match app_timing::record(database, run) {
        Ok(()) => return Ok(()),
        Err(refused) => refused,
    };
    if !install_ran()? {
        return Err(refused);
    }
    let own = folder.join(crate::text::staged_suffix());
    std::fs::create_dir_all(&own).context(|| format!("creating {}", own.display()))?;
    let store = own.join(DATABASE);
    let handed = (|| {
        drop(crate::db::open(&store, crate::db::Make::Allowed)?);
        app_timing::record(&store, run)?;
        hand_off(&store)
    })();
    let removed = std::fs::remove_dir_all(&own).context(|| format!("deleting {}", own.display()));
    crate::fail::with_cleanup(handed, removed)
}

/// Runs the builds that install put on PATH, <installed>, to record the
/// runs of the handed store <store> for <repo>.
pub fn hand_to(installed: &Path, repo: &str, store: &Path) -> Result<()> {
    let status = std::process::Command::new(installed)
        .args(["app-build", FLAG, repo])
        .arg(store)
        .stdin(std::process::Stdio::null())
        .status()
        .context(|| format!("running {}", installed.display()))?;
    if !status.success() {
        fail!(
            "{} could not record the run that this lane handed it ({status})",
            installed.display()
        );
    }
    Ok(())
}

/// `builds app-build --record-handed <repo> <store>`: moves the handed store
/// <store> to this program's schema and records each run it holds in the
/// store of <repo>'s project, at <now>.
pub fn receive(places: &Places, repo: &str, store: &Path, now: i64) -> Result<()> {
    crate::migrate::move_store(places, store, now)?;
    let runs = app_timing::read_runs(store)?;
    let Some(database) = app_timing::database_of(places, repo)? else {
        fail!("repo {repo} is in no project, so its runs have no store to go to");
    };
    for run in &runs {
        app_timing::record(&database, run)?;
    }
    Ok(())
}

/// `builds app-build --record-handed` with its arguments <args>.
pub fn cmd_record_handed(places: &Places, args: &[String]) -> Result<()> {
    let [repo, store] = args else {
        fail!("usage: builds app-build {FLAG} <repo> <store>");
    };
    receive(places, repo, Path::new(store), app_timing::now())
}

#[cfg(test)]
#[path = "handed_runs_tests.rs"]
mod tests;
