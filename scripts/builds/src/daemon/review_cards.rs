//! Where the rules review of a worktree's commits stands, for its card: the
//! commits waiting to land are judged the way `builds land` judges them
//! (`review::landing`), so a card says Ready to land only when land would take
//! them. A review that runs now over the branch's newest commit shows as
//! running.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::cards::{ItemWait, ReviewWait};
use crate::fail::Fail;
use crate::lists::store::Store;
use crate::review::commits;
use crate::review::landing::{self, CodeCheck, Need, Verdict};
use crate::review::run::Material;
use crate::side_git;

/// How long the rules read from the rules checkout are used before they are
/// read again.
const RULES_FOR: Duration = Duration::from_secs(10);

/// A worktree and its commits (full hashes, oldest first).
type ReadKey = (PathBuf, Vec<String>);

/// The paths each commit of a worktree changes.
type Read = Vec<Vec<String>>;

/// A project's rules as `Material::load` last read them, and when; None when
/// `Material::load` failed.
type Rules = (Instant, Option<Arc<Material>>);

/// What git said of each worktree's commits, kept until they differ, and the
/// rules of each project as last read.
pub struct Reads {
    /// The rules checkout; None when HOME is not set.
    root: Option<PathBuf>,
    commits: Mutex<HashMap<ReadKey, Read>>,
    rules: Mutex<HashMap<String, Rules>>,
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

    /// The paths each of <shas> changes in <top>, or why git could not read
    /// them. Reads keeps only a read that succeeded, so the next page build
    /// asks git again after a failure.
    fn commits(&self, top: &Path, shas: &[String]) -> Result<Read, String> {
        let key = (top.to_path_buf(), shas.to_vec());
        let mut known = self.commits.lock().expect("no panic holding it");
        if let Some(found) = known.get(&key) {
            return Ok(found.clone());
        }
        let read = commits::paths_of_each(top, shas).map_err(|why| {
            format!(
                "reading the paths the commits of {} change: {}",
                top.display(),
                why.text()
            )
        })?;
        if known.len() > 256 {
            known.clear();
        }
        known.insert(key, read.clone());
        Ok(read)
    }

    /// Each of <commits>, which change <paths>, with the names of
    /// <project>'s groups whose rules cover a path it changes, and the rules
    /// it read them from; None when the rules can't be read.
    fn needs(
        &self,
        project: &str,
        shas: &[String],
        paths: &[Vec<String>],
    ) -> Option<(Vec<Need>, Arc<Material>)> {
        let root = self.root.as_ref()?;
        let mut rules = self.rules.lock().expect("no panic holding it");
        let fresh = rules
            .get(project)
            .is_some_and(|(at, _)| at.elapsed() < RULES_FOR);
        if !fresh {
            rules.insert(
                project.to_string(),
                (
                    Instant::now(),
                    Material::load(root, project).ok().map(Arc::new),
                ),
            );
        }
        let material = rules.get(project)?.1.clone()?;
        let needs = shas
            .iter()
            .zip(paths)
            .map(|(sha, paths)| Need {
                sha: sha.clone(),
                paths: paths.clone(),
                groups: material.group_names(paths),
            })
            .collect();
        Some((needs, material))
    }

    /// Where the review of <top>'s commits stands, for a worktree whose
    /// project's store is <store> and whose commits not on origin/main
    /// are <ahead> (newest first); None when land would take them as far as
    /// the review goes, or when they need no review. Fails, saying why, when
    /// git cannot read the commits or builds cannot open or read the store.
    pub fn standing(
        &self,
        store: &Store,
        top: &Path,
        ahead: &[side_git::Commit],
    ) -> Result<Option<ItemWait>, String> {
        let Some(newest) = ahead.first() else {
            return Ok(None);
        };
        let shas: Vec<String> = ahead.iter().rev().map(|c| c.full.clone()).collect();
        let paths = self.commits(top, &shas)?;
        let Some((needs, material)) = self.needs(&store.project, &shas, &paths) else {
            return Ok(None);
        };
        if needs.iter().all(|n| n.groups.is_empty()) {
            return Ok(None);
        }
        if crate::review::is_running(&store.dir, &newest.full) {
            return Ok(Some(ItemWait::RulesReview(ReviewWait::Running)));
        }
        let conn = store.open_existing().map_err(Fail::text)?;
        let key = crate::review::default_key(top).map_err(Fail::text)?;
        let check = CodeCheck::Branch("HEAD");
        let groups_for = |paths: &[String]| material.group_names(paths);
        let wait = match landing::judge(&conn, &key, top, &needs, check, &groups_for)
            .map_err(Fail::text)?
        {
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
            Verdict::Unfinished(_, unfinished) => ReviewWait::Open(unfinished.open.len()),
        };
        Ok(Some(ItemWait::RulesReview(wait)))
    }
}

#[cfg(test)]
#[path = "review_cards_tests.rs"]
mod tests;
