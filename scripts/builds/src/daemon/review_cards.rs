//! Where the rules review of a worktree's commits stands, for its card: the
//! commits waiting to land are judged the way `builds land` judges them
//! (`review::landing`), so a card says Ready to land only when land would take
//! them. A review that runs now over the branch's newest commit shows as
//! running.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::cards::{ItemWait, ReviewWait};
use crate::fail::Fail;
use crate::lists::store::Store;
use crate::review::commits::{self, Commit};
use crate::review::landing::{self, CodeCheck, Need, Verdict};
use crate::review::run::Material;
use crate::side_git;

/// How long the rules read from the rules checkout are used before they are
/// read again.
const RULES_FOR: Duration = Duration::from_secs(10);

/// A worktree and its commits (full hashes, oldest first).
type ReadKey = (PathBuf, Vec<String>);

/// The commits of a worktree with their patch ids, and the paths each changes.
type Read = Option<(Vec<Commit>, Vec<Vec<String>>)>;

/// What git said of each worktree's commits, kept until they differ, and the
/// rules of each project as last read.
pub struct Reads {
    /// The rules checkout; None when HOME is not set.
    root: Option<PathBuf>,
    commits: Mutex<HashMap<ReadKey, Read>>,
    rules: Mutex<HashMap<String, (Instant, Option<Material>)>>,
}

impl Default for Reads {
    fn default() -> Reads {
        Reads::with_root(crate::review::default_rules_root().ok())
    }
}

impl Reads {
    /// Reads that take the rules from the checkout at <root>.
    pub fn with_root(root: Option<PathBuf>) -> Reads {
        Reads {
            root,
            commits: Mutex::new(HashMap::new()),
            rules: Mutex::new(HashMap::new()),
        }
    }

    fn commits(&self, top: &Path, shas: &[String]) -> Read {
        let key = (top.to_path_buf(), shas.to_vec());
        let mut known = self.commits.lock().expect("no panic holding it");
        if let Some(found) = known.get(&key) {
            return found.clone();
        }
        // A worktree that stops existing or a failed git read says nothing;
        // the next page build asks again.
        let read = commits::with_patch_ids(top, shas)
            .and_then(|with| {
                let paths = commits::paths_of_each(top, &with)?;
                Ok((with, paths))
            })
            .ok();
        if known.len() > 256 {
            known.clear();
        }
        known.insert(key, read.clone());
        read
    }

    /// Each of <commits>, which change <paths>, with the names of
    /// <project>'s groups whose rules cover a path it changes; None when the
    /// rules can't be read.
    fn needs(&self, project: &str, commits: &[Commit], paths: &[Vec<String>]) -> Option<Vec<Need>> {
        let root = self.root.as_ref()?;
        let mut rules = self.rules.lock().expect("no panic holding it");
        let fresh = rules
            .get(project)
            .is_some_and(|(at, _)| at.elapsed() < RULES_FOR);
        if !fresh {
            rules.insert(
                project.to_string(),
                (Instant::now(), Material::load(root, project).ok()),
            );
        }
        let material = rules.get(project)?.1.as_ref()?;
        Some(
            commits
                .iter()
                .zip(paths)
                .map(|(commit, paths)| Need {
                    commit: commit.clone(),
                    paths: paths.clone(),
                    groups: material.group_names(paths),
                })
                .collect(),
        )
    }

    /// Where the review of <top>'s commits stands, for a worktree of <repo>
    /// whose project's store is <store> and whose commits not on origin/main
    /// are <ahead> (newest first); None when land would take them as far as
    /// the review goes, or when they need no review. Fails, saying why, when
    /// it can't open or read the store.
    pub fn standing(
        &self,
        store: &Store,
        repo: &str,
        top: &Path,
        ahead: &[side_git::Commit],
    ) -> Result<Option<ItemWait>, String> {
        let Some(newest) = ahead.first() else {
            return Ok(None);
        };
        let shas: Vec<String> = ahead.iter().rev().map(|c| c.full.clone()).collect();
        let Some((commits, paths)) = self.commits(top, &shas) else {
            return Ok(None);
        };
        let Some(needs) = self.needs(&store.project, &commits, &paths) else {
            return Ok(None);
        };
        if needs.iter().all(|n| n.groups.is_empty()) {
            return Ok(None);
        }
        if crate::review::is_running(&store.dir, &newest.full) {
            return Ok(Some(ItemWait::RulesReview(ReviewWait::Running)));
        }
        let conn = store.open_existing().map_err(Fail::text)?;
        // The card only reads the store. The card counts a finding whose code
        // is no longer in the branch's change as blocking nothing, as land does
        // when it records the finding.
        let check = CodeCheck::Branch("HEAD");
        let wait = match landing::judge(&conn, repo, top, &needs, check).map_err(Fail::text)? {
            Verdict::NotNeeded | Verdict::Takes(_) => return Ok(None),
            Verdict::NoRun => ReviewWait::NotRun,
            Verdict::Unfinished(_, unfinished) if !unfinished.unsettled.is_empty() => {
                ReviewWait::ToSettle(unfinished.unsettled.len())
            }
            Verdict::Unfinished(_, unfinished) if !unfinished.unanswered.is_empty() => {
                ReviewWait::NotRun
            }
            Verdict::Unfinished(_, unfinished) if !unfinished.unreviewed.is_empty() => {
                ReviewWait::Unreviewed(unfinished.unreviewed.len())
            }
            Verdict::Unfinished(_, unfinished) => {
                ReviewWait::Unfixed(unfinished.unfixed.len() + unfinished.fixed_elsewhere.len())
            }
        };
        Ok(Some(ItemWait::RulesReview(wait)))
    }
}

#[cfg(test)]
#[path = "review_cards_tests.rs"]
mod tests;
