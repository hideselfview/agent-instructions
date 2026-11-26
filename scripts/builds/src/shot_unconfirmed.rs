//! Marks the findings of a cached run that other cached runs do not show, so
//! that no one queues work from one run's flaky shot.
//!
//! Two runs of a scene measure the same thing when they were made in the same
//! environment by the same shots program, and the scene's board and the
//! lane's app code are the same in both: the board's hash in each run's
//! findings.json (`Run::boards`) is equal, and so is the app source tree
//! (app_tree.rs) of the runs' commits. A finding of the run is unconfirmed
//! when at least one other such run of its scene lacks the same finding (the
//! same level and sentence). A scene no other run measures the same way has
//! no mark: nothing says its findings are flaky.

use std::collections::{BTreeMap, BTreeSet};

use crate::app_config::config_in;
use crate::app_remote::fetched_local_source;
use crate::app_tree;
use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::places::Places;
use crate::shot_causes::{self, Others, Run};
use crate::shots_cache::{self, Cached};
use crate::text::{note, plural};

/// A finding as another run shows it: its scene, level and sentence.
type Shown = (String, String, String);

/// What the other cached runs show of <cached>'s findings, which <run> holds:
/// sets `others` on each record of a scene that another run measures the
/// same way. Says on stderr how many runs it compared with, and how many it
/// could not place.
pub fn mark(places: &Places, cached: &Cached, run: &mut Run) -> Result<()> {
    if run.findings.is_empty() {
        return Ok(());
    }
    let key = &cached.key;
    let _reading = shots_cache::writing(places)?;
    let mut trees = AppTrees::new(places, &key.repo, key.lane)?;
    let Some(own_tree) = trees.of(&key.commit)? else {
        fail!(
            "the app checkout of {} lacks commit {}, so no other run can be compared with its app code",
            key.repo,
            key.commit
        );
    };
    let mut comparable = Vec::new();
    let mut unplaced = 0;
    for other in shots_cache::runs_in_environment(
        places,
        &key.repo,
        key.lane,
        &key.environment,
        &key.versions,
    )? {
        if other.folder == cached.folder {
            continue;
        }
        let theirs = shot_causes::read(&other.report())?;
        if boards_in_common(run, &theirs).next().is_none() {
            continue;
        }
        if other.key.commit != key.commit {
            match trees.of(&other.key.commit)? {
                Some(tree) if tree == own_tree => {}
                Some(_) => continue,
                None => {
                    unplaced += 1;
                    continue;
                }
            }
        }
        comparable.push(theirs);
    }
    apply(run, &comparable);
    note(format!(
        "compared with {} made in the same environment with the same board and app code{}",
        plural(comparable.len(), "other run"),
        if unplaced == 0 {
            String::new()
        } else {
            format!(
                "; {} of a commit the app checkout lacks could not be placed",
                plural(unplaced, "run")
            )
        }
    ));
    Ok(())
}

/// The scenes with findings in <run> that <theirs> checked with the same
/// board.
fn boards_in_common<'a>(run: &'a Run, theirs: &'a Run) -> impl Iterator<Item = &'a str> {
    let scenes: BTreeSet<&str> = run.findings.iter().map(|r| r.scene.as_str()).collect();
    scenes.into_iter().filter(|scene| {
        run.boards
            .get(*scene)
            .is_some_and(|hash| theirs.boards.get(*scene) == Some(hash))
    })
}

/// Sets `others` on each record of <run> whose scene some run of <others>
/// checked with the same board, <others> being runs of the same app code.
fn apply(run: &mut Run, others: &[Run]) {
    // For each scene, how many of the other runs measure it the same way,
    // and for each finding, how many of them show it.
    let mut measured: BTreeMap<String, usize> = BTreeMap::new();
    let mut shown: BTreeMap<Shown, usize> = BTreeMap::new();
    for theirs in others {
        let scenes: BTreeSet<&str> = boards_in_common(run, theirs).collect();
        for scene in &scenes {
            *measured.entry(scene.to_string()).or_default() += 1;
        }
        for record in theirs
            .findings
            .iter()
            .filter(|r| scenes.contains(r.scene.as_str()))
        {
            *shown.entry(shown_of(record)).or_default() += 1;
        }
    }
    for record in &mut run.findings {
        let of = measured.get(&record.scene).copied().unwrap_or(0);
        if of > 0 {
            let with = shown.get(&shown_of(record)).copied().unwrap_or(0);
            record.others = Some(Others {
                of,
                without: of - with,
            });
        }
    }
}

fn shown_of(record: &shot_causes::Record) -> Shown {
    (
        record.scene.clone(),
        record.level.clone(),
        record.sentence.clone(),
    )
}

/// The app source trees of a repo's commits on one lane, read from the repo's
/// app checkout.
struct AppTrees {
    top: std::path::PathBuf,
    lane: Lane,
    known: BTreeMap<String, String>,
}

impl AppTrees {
    fn new(places: &Places, repo: &str, lane: Lane) -> Result<Self> {
        Ok(Self {
            top: fetched_local_source(places, repo)?,
            lane,
            known: BTreeMap::new(),
        })
    }

    /// The app source tree of <commit>; none when the app checkout lacks the
    /// commit. Any other failure fails the read.
    fn of(&mut self, commit: &str) -> Result<Option<String>> {
        if let Some(tree) = self.known.get(commit) {
            return Ok(Some(tree.clone()));
        }
        if !crate::git::commit_exists(&self.top, commit)? {
            return Ok(None);
        }
        let config = config_in(&self.top, commit)?;
        let tree = app_tree::of(&self.top, commit, &config, self.lane)?;
        self.known.insert(commit.to_string(), tree.clone());
        Ok(Some(tree))
    }
}

#[cfg(test)]
#[path = "shot_unconfirmed_tests.rs"]
mod tests;
