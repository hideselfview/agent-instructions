//! The record of an app build lane's run when an install moved the stores
//! while the lane built.
//!
//! An install never stops an app build: each lane builds to its end on the
//! programs it started with (programs.rs) and writes its run when it ends
//! (app_timing.rs). An install that changed a store's schema meanwhile moved
//! the stores to the installed program's schema, which this program cannot
//! read or write. The lane writes its run to a new store of its own schema
//! under the install folder and runs the installed builds
//! (`builds app-build --record-handed <repo> <store>`), which moves that
//! store to its schema with the moves it makes for every store (migrate.rs),
//! reads the run back, finds the repo's project and records the run there.
//! Then the lane deletes the folder. The run reaches the store whole or the
//! record fails, as any record does.

use std::path::{Path, PathBuf};

use crate::app_timing::{self, DATABASE, RunRecord};
use crate::fail;
use crate::fail::{Context, Result};
use crate::install_gate::{self, ConnectionUse};
use crate::places::Places;

/// The `builds app-build` flag of the form that records a handed store.
pub const FLAG: &str = "--record-handed";

/// The folder of the stores that lanes hand the installed builds.
fn folder(places: &Places) -> PathBuf {
    places.install_dir().join("handed-runs")
}

/// Records <run> once the lane's steps have ended. The use prevents a schema
/// move between choosing the writer and committing the run.
pub fn record(places: &Places, run: &RunRecord) -> Result<()> {
    let using_stores = install_gate::for_connection()?;
    let installed = crate::links::path(&places.home, "builds");
    write(places, run, install_gate::install_ran, |store| {
        hand_to(&installed, &run.repo, store, using_stores.as_ref())
    })
}

/// Writes <run> through the installed program when an install moved the
/// stores since the lane started. Project lookup also needs the store's
/// schema, so only the program that will write the run looks up its project.
fn write(
    places: &Places,
    run: &RunRecord,
    install_ran: impl FnOnce() -> Result<bool>,
    hand_off: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if !install_ran()? {
        return record_in_project(places, &run.repo, run);
    }
    let own = folder(places).join(crate::text::staged_suffix());
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
fn hand_to(
    installed: &Path,
    repo: &str,
    store: &Path,
    using_stores: Option<&ConnectionUse>,
) -> Result<()> {
    let mut command = std::process::Command::new(installed);
    command
        .args(["app-build", FLAG, repo])
        .arg(store)
        .stdin(std::process::Stdio::null());
    if let Some(using_stores) = using_stores {
        using_stores.cover(&mut command);
    }
    let status = command
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
/// <store> to this program's schema and records the run it holds in the
/// store of <repo>'s project, at <now>.
fn receive(places: &Places, repo: &str, store: &Path, now: i64) -> Result<()> {
    let _using_stores = install_gate::for_connection()?;
    crate::migrate::move_store(places, store, now)?;
    let run = app_timing::read_run(store)?;
    record_in_project(places, repo, &run)
}

fn record_in_project(places: &Places, repo: &str, run: &RunRecord) -> Result<()> {
    if let Some(database) = app_timing::database_of(places, repo)? {
        app_timing::record(&database, run)?;
    }
    Ok(())
}

/// `builds app-build --record-handed` with its arguments <args>.
pub fn cmd_record_handed(places: &Places, args: &[String]) -> Result<()> {
    let [repo, store] = args else {
        fail!("usage: builds app-build {FLAG} <repo> <store>");
    };
    let path =
        std::path::absolute(store).context(|| format!("resolving the handed store at {store}"))?;
    receive(places, repo, &path, app_timing::now())
}

#[cfg(test)]
#[path = "handed_runs_tests.rs"]
mod tests;
