//! Which worktrees hold a design branch and where its design change stands:
//! for each commit of a worktree that changes design files, an open design
//! change covers it or none does, as `design::design_commits` decides for
//! health and land. A worktree with a commit whose covering change waits
//! on the person's review waits on that review; one with a commit no change
//! covers waits until someone records one: the design session, or the
//! implementing agent for a board edit of a change the person decided.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::cards::ItemWait;
use crate::fail::Result;
use crate::lists::design::{design_commits, open_designs};
use crate::lists::design_paths::Globs;
use crate::lists::{Id, Lists};
use crate::side_git::Commit;

/// Which open design change covers each design commit of a worktree, kept
/// until its commits, the repo's globs or its open design changes differ.
#[derive(Default)]
pub struct Reads {
    coverage: Mutex<HashMap<ReadKey, Vec<Option<Id>>>>,
}

/// A worktree, its commits (full hashes, oldest first), and the repo's globs
/// and open design changes as text.
type ReadKey = (PathBuf, Vec<String>, String);

impl Reads {
    /// The covering change of each design commit of <shas> in <top>, a
    /// worktree of <repo>, by id; None for a commit no change covers.
    fn coverage(
        &self,
        lists: &Lists,
        repo: &str,
        top: &Path,
        shas: &[String],
    ) -> Result<Vec<Option<Id>>> {
        let designs: Vec<_> = open_designs(lists, repo).collect();
        let key = (
            top.to_path_buf(),
            shas.to_vec(),
            format!("{:?} {designs:?}", Globs::of(lists, repo)),
        );
        let mut known = self.coverage.lock().expect("no panic holding it");
        if let Some(found) = known.get(&key) {
            return Ok(found.clone());
        }
        // A failed git read is not kept, so the next page build reads again.
        let read: Vec<Option<Id>> = design_commits(lists, repo, top, shas)?
            .iter()
            .map(|commit| commit.covered_by.map(|design| design.id))
            .collect();
        if known.len() > 256 {
            known.clear();
        }
        known.insert(key, read.clone());
        Ok(read)
    }

    /// The standing of the design branch in <top>, a worktree of <repo>
    /// whose commits not on origin/main are <ahead> (newest first); None
    /// when the commits change no design files or approved changes cover
    /// them. Fails when git can't read the commits or a change's last commit.
    pub fn standing(
        &self,
        lists: &Lists,
        repo: &str,
        top: &Path,
        ahead: &[Commit],
    ) -> Result<Option<ItemWait>> {
        let shas: Vec<String> = ahead.iter().rev().map(|c| c.full.clone()).collect();
        let coverage = self.coverage(lists, repo, top, &shas)?;
        let reviewing = coverage
            .iter()
            .flatten()
            .filter_map(|id| lists.design(*id))
            .find(|design| design.waits_on_person());
        Ok(match reviewing {
            Some(design) => Some(ItemWait::Review {
                id: design.id.0,
                title: design.title.clone(),
            }),
            None if coverage.iter().any(Option::is_none) => Some(ItemWait::Unrecorded),
            None => None,
        })
    }
}

#[cfg(test)]
#[path = "design_cards_tests.rs"]
mod tests;
