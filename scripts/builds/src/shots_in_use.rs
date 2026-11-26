//! The cached shots runs (shots_cache.rs) that a branch waiting to land still
//! needs. A branch with commits origin/main lacks lands only when its app
//! build's shots runs compare with the run of the build's merge base, and
//! `builds land` reads the run of the passing build that covers the branch
//! (app_landing.rs). So a branch names two runs for each shots prefix of each
//! of its app builds: the run of the build's commit, which covers the prefix,
//! and the run of that commit's merge base with origin/main, which the run
//! compared with, made in any environment. The cap never deletes a run that a
//! branch names (capped_store.rs `Kept::InUse`).
//!
//! Which app builds a branch made is in the project's store
//! (app_timing.sql); which branches wait to land is in git.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::fail;
use crate::fail::Result;
use crate::places::Places;
use crate::side_git::MainCopies;

/// The folder of each cached run that a branch names, with the branches that
/// name it.
pub type Named = BTreeMap<PathBuf, BTreeSet<String>>;

/// The runs of <repo> that a branch waiting to land names, through the app
/// builds of every project that holds <repo>, since the repo has one folder
/// of cached runs; nothing when no project's store holds <repo>, which
/// records no app builds.
pub fn named(places: &Places, repo: &str) -> Result<Named> {
    let mut named = Named::new();
    let mut source = None;
    for path in crate::lists::store::databases_holding(places, repo)? {
        let Some(conn) = crate::db::open_reader(&path)? else {
            continue;
        };
        let source = match &source {
            Some(source) => source,
            None => source.insert(crate::app_remote::local_source(places, repo)?),
        };
        for (folder, branches) in named_in(places, repo, source, &conn, &path)? {
            named.entry(folder).or_default().extend(branches);
        }
    }
    Ok(named)
}

/// `named`, reading the branches from the checkout <source> and the app
/// builds from the database open as <conn> at <path>.
pub fn named_in(
    places: &Places,
    repo: &str,
    source: &Path,
    conn: &Connection,
    path: &Path,
) -> Result<Named> {
    let waiting = waiting_branches(source)?;
    let mut named = Named::new();
    let Some(since) = waiting.values().min() else {
        return Ok(named);
    };
    let versions = &crate::shots_program::VERSIONS;
    let mut bases: HashMap<String, Option<String>> = HashMap::new();
    let lanes = crate::repo_lanes::in_source(source)?;
    for build in crate::app_timing::shots_builds(conn, repo, *since, path)? {
        match waiting.get(&build.branch) {
            Some(first) if build.started_at >= *first => {}
            _ => continue,
        }
        if !bases.contains_key(&build.commit) {
            let base = merge_base_of(source, &build.commit)?;
            bases.insert(build.commit.clone(), base);
        }
        let base = bases[&build.commit].as_ref();
        let lane = crate::repo_lanes::parse_among(&lanes, &build.lane)?;
        for commit in std::iter::once(&build.commit).chain(base) {
            for folder in crate::shots_cache::covering_folders(
                places,
                repo,
                commit,
                lane,
                &build.prefix,
                versions,
            )? {
                named
                    .entry(folder)
                    .or_default()
                    .insert(build.branch.clone());
            }
        }
    }
    Ok(named)
}

/// The merge base of <commit> with origin/main in <source>, when it differs
/// from <commit>; None for a commit on origin/main and for one that git no
/// longer has, whose runs no landing compares.
fn merge_base_of(source: &Path, commit: &str) -> Result<Option<String>> {
    if !crate::git::commit_exists(source, commit)? {
        return Ok(None);
    }
    let base = crate::derive::merge_base(source, commit)?;
    Ok((base != commit).then_some(base))
}

/// Each local branch of <source> that holds commits origin/main lacks, with
/// the author time of its first such commit, which an app build of the
/// branch started after.
fn waiting_branches(source: &Path) -> Result<BTreeMap<String, i64>> {
    let branches: Vec<String> = crate::unlanded::unmerged_branches(source)?
        .into_iter()
        .map(|(branch, _)| branch)
        .collect();
    if branches.is_empty() {
        return Ok(BTreeMap::new());
    }
    let copies = MainCopies::all(source).map_err(fail::Fail::Message)?;
    let found = crate::checkout::parallel_map(&branches, |branch| {
        crate::side_git::waiting_to_land_beside(source, &format!("refs/heads/{branch}"), &copies)
    });
    let mut waiting = BTreeMap::new();
    for (branch, commits) in branches.iter().zip(found) {
        let commits = commits.map_err(fail::Fail::Message)?;
        if let Some(first) = commits.iter().map(|commit| commit.authored).min() {
            waiting.insert(branch.clone(), first);
        }
    }
    Ok(waiting)
}

#[cfg(test)]
#[path = "shots_in_use_tests.rs"]
mod tests;
