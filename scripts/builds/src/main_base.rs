//! The run that an app build's shots compare with when its branch has no
//! cached run of its own merge base with origin/main.
//!
//! A shots run of a branch compares with the cached run of the branch's merge
//! base (shots_cache.rs `before_run`). A merge base with no cached run makes
//! builds run, at the tip of origin/main, the prefixes the build asks for and
//! the tip's cache lacks, which every build of the lane shares, and a branch
//! that starts behind that tip rebases onto it before it compares: a branch
//! that lacks main's newer commits would show main's fixes as findings it
//! adds. A prefix that starts no shot the tip declares needs no run there:
//! every scene it selects on the branch is new.

use std::path::Path;

use crate::app_build::Pick;
use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::places::Places;
use crate::shots_compare_again::before_run;

/// What a build does to get the runs its shots compare with.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// The merge base has a run for every shots pick, or the picks that lack
    /// one start no shot the tip declares, so nothing exists to compare them
    /// with.
    Covered,
    /// The merge base has no run of these picks, which start declared shots
    /// at origin/main's tip, and the tip's cache lacks their runs: builds
    /// runs them at the tip.
    BuildMain(Vec<Pick>),
    /// origin/main's tip has the runs and the branch starts behind it: the
    /// branch rebases onto the tip.
    Rebase,
}

/// The distinct lanes of <picks>, in the order they first appear.
pub fn lanes_of(picks: &[Pick]) -> Vec<Lane> {
    let mut lanes: Vec<Lane> = Vec::new();
    for (lane, _, _) in picks {
        if !lanes.contains(lane) {
            lanes.push(*lane);
        }
    }
    lanes
}

/// The first 12 characters of a commit.
pub fn short_of(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// The picks of <picks> that have no run covering them at <commit>.
fn missing(
    places: &Places,
    repo: &str,
    commit: &str,
    picks: &[Pick],
    program: &Path,
) -> Result<Vec<Pick>> {
    let mut missing = Vec::new();
    for pick in picks {
        let (lane, _, prefix) = pick;
        if before_run(places, repo, commit, *lane, prefix, program)?.is_none() {
            missing.push(pick.clone());
        }
    }
    Ok(missing)
}

/// What a build of a branch whose merge base with origin/main is <base> does
/// to compare its shots <picks> with a run, when origin/main's tip is <tip>.
/// <declared> gives the names of the shots the tip declares, and is called
/// only when the merge base lacks a run. Fails when the tip is the merge base
/// and still holds no run after it was built: nothing then explains the gap.
pub fn plan(
    places: &Places,
    repo: &str,
    base: &str,
    tip: &str,
    picks: &[Pick],
    declared: impl FnOnce() -> Result<Vec<String>>,
    program: &Path,
) -> Result<Plan> {
    let lacking = missing(places, repo, base, picks, program)?;
    if lacking.is_empty() {
        return Ok(Plan::Covered);
    }
    let declared = declared()?;
    let asked: Vec<Pick> = lacking
        .into_iter()
        .filter(|(_, _, prefix)| starts_a_shot(&declared, prefix))
        .collect();
    let absent = missing(places, repo, tip, &asked, program)?;
    if !absent.is_empty() {
        return Ok(Plan::BuildMain(absent));
    }
    if asked.is_empty() {
        return Ok(Plan::Covered);
    }
    if base == tip {
        fail!(
            "origin/main's tip {} holds the runs of the picks, yet the merge base has none",
            short_of(tip)
        );
    }
    Ok(Plan::Rebase)
}

/// Whether a name of <declared> starts with <prefix>.
pub fn starts_a_shot(declared: &[String], prefix: &str) -> bool {
    declared.iter().any(|name| name.starts_with(prefix))
}

/// Takes the exclusive lock on <lane>'s runs of <repo>'s main, waiting while
/// another build holds it, so that builds run a prefix at main's tip once while the
/// others wait for it and then find it cached; it lasts as long as the file
/// it returns.
pub fn lock(places: &Places, repo: &str, lane: Lane) -> Result<std::fs::File> {
    let folder = places.root.join(repo);
    std::fs::create_dir_all(&folder).map_err(|error| {
        crate::fail::Fail::Message(format!("creating {}: {error}", folder.display()))
    })?;
    let path = folder.join(format!("main-run-{}.lock", lane.word()));
    crate::evict::lock_exclusive_creating(&path)
}

#[cfg(test)]
#[path = "main_base_tests.rs"]
mod tests;
