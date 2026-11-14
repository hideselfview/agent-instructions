//! Which worktrees hold a design branch and where its design change stands:
//! the commits of a worktree are compared with the places of the design
//! changes recorded for it. A worktree whose commits change design files
//! and whose recorded, unapproved design change names every one of those
//! places waits on the person's review; one with no such change waits on the
//! design session to record it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::cards::ItemWait;
use crate::lists::Lists;
use crate::lists::design_paths::{Changes, Globs, changes_of};
use crate::side_git::Commit;

/// What git said the commits of a worktree change, kept until its commits
/// or the repo's design globs differ.
#[derive(Default)]
pub struct Reads {
    changes: Mutex<HashMap<ReadKey, Option<Changes>>>,
}

/// A worktree, its commits (full hashes, oldest first) and the repo's globs.
type ReadKey = (PathBuf, Vec<String>, String);

impl Reads {
    fn changes(&self, top: &Path, globs: &Globs, shas: &[String]) -> Option<Changes> {
        let key = (top.to_path_buf(), shas.to_vec(), format!("{globs:?}"));
        let mut known = self.changes.lock().expect("no panic holding it");
        if let Some(found) = known.get(&key) {
            return found.clone();
        }
        // A worktree that stops existing or a failed git read says nothing
        // about design; the next page build asks again.
        let read = changes_of(top, globs, shas).ok();
        if known.len() > 256 {
            known.clear();
        }
        known.insert(key, read.clone());
        read
    }

    /// The standing of the design branch in <top>, a worktree of <repo>
    /// whose commits not on origin/main are <ahead> (newest first); None
    /// when the commits change no design files or an approved change covers
    /// them.
    pub fn standing(
        &self,
        lists: &Lists,
        repo: &str,
        top: &Path,
        ahead: &[Commit],
    ) -> Option<ItemWait> {
        if ahead.is_empty() {
            return None;
        }
        let globs = Globs::of(lists, repo);
        let shas: Vec<String> = ahead.iter().rev().map(|c| c.full.clone()).collect();
        let changes = self.changes(top, &globs, &shas)?;
        if !changes.is_design_change() {
            return None;
        }
        let here = top.display().to_string();
        let covering: Vec<_> = lists
            .designs
            .iter()
            .filter(|d| d.repo == repo && d.worktree == here)
            .filter(|d| {
                changes
                    .resolvable()
                    .iter()
                    .all(|p| d.places.contains_key(*p))
                    && changes.deleted.iter().all(|p| d.deleted.contains(p))
            })
            .collect();
        match covering.iter().find(|d| d.approved.is_none()) {
            Some(design) => Some(ItemWait::Review {
                id: design.id.0,
                title: design.title.clone(),
            }),
            None if covering.is_empty() => Some(ItemWait::Unrecorded),
            None => None,
        }
    }
}

#[cfg(test)]
#[path = "design_cards_tests.rs"]
mod tests;
