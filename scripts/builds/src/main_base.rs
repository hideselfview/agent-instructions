//! The run that an app build's shots compare with when its branch has no
//! cached run of its own merge base with origin/main.
//!
//! A shots run of a branch compares with the cached run of the branch's merge
//! base (shots_cache.rs `before_run`). Branches start from many different
//! main commits, so building each branch's old merge base for the comparison
//! made most first builds two builds. Instead, a merge base with no cached run
//! makes builds run the whole lane's shots once at the tip of origin/main,
//! which every later build of the lane shares, and a branch that starts behind
//! that tip rebases onto it before it compares: a branch that lacks main's
//! newer commits would show main's fixes as findings it adds.

use std::path::Path;

use crate::app_build::{Pick, all_shots};
use crate::fail;
use crate::fail::Result;
use crate::flock;
use crate::lane::Lane;
use crate::places::Places;
use crate::shots_compare_again::before_run;

/// What a build does to get the runs its shots compare with.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// The merge base has a run for every shots pick.
    Covered,
    /// The merge base has none, and origin/main's tip lacks the whole-lane run
    /// of these lanes: builds runs them at the tip.
    BuildMain(Vec<Pick>),
    /// origin/main's tip has the runs and the branch starts behind it: the
    /// branch rebases onto the tip.
    Rebase,
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
/// Fails when the tip is the merge base and still holds no run after it was
/// built: nothing then explains the gap.
pub fn plan(
    places: &Places,
    repo: &str,
    base: &str,
    tip: &str,
    picks: &[Pick],
    program: &Path,
) -> Result<Plan> {
    if missing(places, repo, base, picks, program)?.is_empty() {
        return Ok(Plan::Covered);
    }
    let mut lanes: Vec<Lane> = Vec::new();
    for (lane, _, _) in picks {
        if !lanes.contains(lane) {
            lanes.push(*lane);
        }
    }
    let whole: Vec<Pick> = lanes.into_iter().map(all_shots).collect();
    let absent = missing(places, repo, tip, &whole, program)?;
    if !absent.is_empty() {
        return Ok(Plan::BuildMain(absent));
    }
    if base == tip {
        fail!(
            "origin/main's tip {} holds the whole-lane run, yet it covers none of the picks",
            &tip[..tip.len().min(12)]
        );
    }
    Ok(Plan::Rebase)
}

/// Takes the exclusive lock on <lane>'s runs of <repo>'s main, waiting while
/// another build holds it, so that builds run main's whole lane once while the
/// others wait for it and then find it cached; it lasts as long as the file
/// it returns.
pub fn lock(places: &Places, repo: &str, lane: Lane) -> Result<std::fs::File> {
    let folder = places.root.join(repo);
    std::fs::create_dir_all(&folder).map_err(|error| {
        crate::fail::Fail::Message(format!("creating {}: {error}", folder.display()))
    })?;
    let path = folder.join(format!("main-run-{}.lock", lane.word()));
    let file = flock::open_creating(&path)?;
    flock::lock(&file, libc::LOCK_EX, &path)?;
    Ok(file)
}

#[cfg(test)]
#[path = "main_base_tests.rs"]
mod tests;
