//! The record of an app build lane's run when an install moved the stores
//! while the lane built.
//!
//! An install never stops an app build: each lane builds to its end on the
//! programs it started with (programs.rs) and writes its run when it ends
//! (app_timing.rs). An install that closed the store gates meanwhile may
//! have moved the stores to a schema this program cannot read or write.
//! The lane then writes its run to a new store of its own schema
//! under the install folder and runs the installed builds
//! (`builds app-build --record-handed <repo> <store> [<project>]`), which
//! moves that store to its schema with the moves it makes for every store
//! (migrate.rs), reads the run back and records it in the store of
//! <project>, the project that records the build; without <project>, in that
//! of the one project that holds the repo.
//! Then the lane deletes the folder. The run reaches the store whole or the
//! record fails, as any record does.

use std::path::{Path, PathBuf};

use crate::app_timing::{self, DATABASE, RunRecord};
use crate::fail;
use crate::fail::{Context, Result};
use crate::install_gate::{self, ConnectionUse};
use crate::lists::store::Store;
use crate::places::Places;

/// The `builds app-build` flag of the form that records a handed store.
pub const FLAG: &str = "--record-handed";

/// The folder of the stores that lanes hand the installed builds.
fn folder(places: &Places) -> PathBuf {
    places.install_dir().join("handed-runs")
}

/// Records <run> in the store of <project>
/// (`app_timing::recording_project`) once the lane's steps have ended. The use
/// prevents a schema move between choosing the writer and committing the run.
pub fn record(places: &Places, run: &RunRecord, project: Option<&str>) -> Result<()> {
    let using_stores = install_gate::for_connection()?;
    let installed = crate::links::path(&places.home, "builds");
    write(places, run, project, install_gate::install_ran, |store| {
        hand_to(&installed, &run.repo, store, project, using_stores.as_ref())
    })
}

/// Writes <run> through the installed program when an install closed the
/// store gates since the lane started. Checking the project also needs the
/// store's schema, so only the program that will write the run checks it.
fn write(
    places: &Places,
    run: &RunRecord,
    project: Option<&str>,
    install_ran: impl FnOnce() -> Result<bool>,
    hand_off: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if !install_ran()? {
        return record_in_project(places, run, project);
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
    project: Option<&str>,
    using_stores: Option<&ConnectionUse>,
) -> Result<()> {
    let mut command = std::process::Command::new(installed);
    command
        .args(["app-build", FLAG, repo])
        .arg(store)
        .args(project)
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

/// `builds app-build --record-handed <repo> <store> [<project>]`: moves the
/// handed store <store> to this program's schema and records the run it holds
/// in the store of <repo>'s project, at <now>.
fn receive(
    places: &Places,
    repo: &str,
    store: &Path,
    project: Option<&str>,
    now: i64,
) -> Result<()> {
    let _using_stores = install_gate::for_connection()?;
    crate::migrate::move_store(places, store, now)?;
    let run = app_timing::read_run(store, |repo, word| {
        crate::repo_lanes::parse(places, repo, word)
    })?;
    if run.repo != repo {
        fail!(
            "The handed run names repo {}, and the caller names {repo}.",
            run.repo
        );
    }
    record_in_project(places, &run, project)
}

/// Records <run> in the store of <project>, which must hold the run's repo;
/// with no <project>, in that of the one project that holds it, and nowhere
/// for a repo in no project.
fn record_in_project(places: &Places, run: &RunRecord, project: Option<&str>) -> Result<()> {
    if let Some(project) = crate::lists::store::project_named(places, &run.repo, project)? {
        app_timing::record(&Store::named(places, &project)?.database(), run)?;
    }
    Ok(())
}

/// `builds app-build --record-handed` with its arguments <args>.
pub fn cmd_record_handed(places: &Places, args: &[String]) -> Result<()> {
    let (repo, store, project) = match args {
        [repo, store] => (repo, store, None),
        [repo, store, project] => (repo, store, Some(project.as_str())),
        _ => fail!("{}", crate::command::Command::AppBuild.usage_text()),
    };
    let path =
        std::path::absolute(store).context(|| format!("resolving the handed store at {store}"))?;
    receive(places, repo, &path, project, app_timing::now())
}

#[cfg(test)]
#[path = "handed_runs_tests.rs"]
mod tests;
