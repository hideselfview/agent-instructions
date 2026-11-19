//! Which worktrees hold a design branch and where its design change stands:
//! for each commit of a worktree that changes design files, an open design
//! change covers it or none does, as `design::design_commits` decides for
//! health and land. A worktree with a commit whose covering change waits
//! on the person's review waits on that review; one with a commit no change
//! covers waits until someone records one: the design session, or the
//! implementing agent for a board edit of a change the person decided.

use std::path::Path;

use super::cards::ItemWait;
use crate::fail::Result;
use crate::lists::Lists;
use crate::lists::design_reads::DesignReads;
use crate::side_git::SideRead;

/// The standing of the design branch in <top>, a worktree of <repo> whose
/// side read is <git>, from the design commits in <reads>; None when the
/// commits change no design files or approved changes cover them. Fails when
/// git can't read the commits or a change's last commit.
pub fn standing(
    reads: &DesignReads,
    lists: &Lists,
    repo: &str,
    top: &Path,
    git: &SideRead,
) -> Result<Option<ItemWait>> {
    let shas: Vec<String> = git.ahead.iter().rev().map(|c| c.full.clone()).collect();
    let commits = reads.of(lists, repo, top, &shas, &git.main)?;
    let reviewing = commits
        .iter()
        .filter_map(|commit| commit.covered_by)
        .filter_map(|id| lists.design(id))
        .find(|design| design.waits_on_person());
    Ok(match reviewing {
        Some(design) => Some(ItemWait::Review {
            id: design.id.0,
            title: design.title.clone(),
        }),
        None if commits.iter().any(|commit| commit.covered_by.is_none()) => {
            Some(ItemWait::Unrecorded)
        }
        None => None,
    })
}

#[cfg(test)]
#[path = "design_cards_tests.rs"]
mod tests;
