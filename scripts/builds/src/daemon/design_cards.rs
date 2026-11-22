//! Which worktrees hold a design branch and where its design change stands:
//! for each commit of a worktree that changes design files, an open design
//! change covers it or none does, as `design::design_commits` decides for
//! health and land. A worktree with a commit whose covering change waits on
//! the person's approval waits on their review once the rules review of the
//! branch takes its commits (decision #362), and on that review until then;
//! one with a commit no change covers waits until someone records one: the
//! design session, or the implementing agent for a board edit of a change the
//! person decided.

use super::cards::ItemWait;
#[cfg(test)]
use crate::fail::Fail;
use crate::lists::design::DesignCommit;
#[cfg(test)]
use crate::lists::design_reads::DesignReads;
use crate::lists::{DesignChange, Lists};
#[cfg(test)]
use crate::side_git::SideRead;
#[cfg(test)]
use std::path::Path;

/// Where the design branch of a worktree stands.
#[derive(Debug)]
pub enum Standing<'a> {
    /// An open design change that waits on the person's approval covers one
    /// of its commits.
    Approval(&'a DesignChange),
    /// No open design change covers one of its commits.
    Unrecorded,
}

/// The standing of the design branch in <top>, a worktree of <repo> whose
/// side read is <git>, from the design commits in <reads>; None when the
/// commits change no design files or approved changes cover them. Fails when
/// git can't read the commits or a change's last commit.
#[cfg(test)]
pub fn standing<'a>(
    reads: &DesignReads,
    lists: &'a Lists,
    repo: &str,
    top: &Path,
    git: &SideRead,
) -> Result<Option<Standing<'a>>, std::sync::Arc<Fail>> {
    let commits = reads.of(lists, repo, top, git)?;
    Ok(of_commits(lists, &commits))
}

pub fn of_commits<'a>(lists: &'a Lists, commits: &[DesignCommit]) -> Option<Standing<'a>> {
    let awaiting = commits
        .iter()
        .filter_map(|commit| commit.covering(lists))
        .find(|design| design.awaits_approval());
    match awaiting {
        Some(design) => Some(Standing::Approval(design)),
        None if commits.iter().any(|commit| commit.covered_by.is_none()) => {
            Some(Standing::Unrecorded)
        }
        None => None,
    }
}

/// What the design branch at <standing> waits on: a change that waits on the
/// person's approval waits on their review once the rules review of the
/// branch takes its commits (<review_takes>), and on nothing of theirs
/// before that, while the card shows the rules review's wait.
pub fn wait(standing: &Standing, review_takes: bool) -> Option<ItemWait> {
    match standing {
        Standing::Approval(design) if review_takes => Some(ItemWait::Review {
            id: design.id.0,
            title: design.title.clone(),
        }),
        Standing::Approval(_) => None,
        Standing::Unrecorded => Some(ItemWait::Unrecorded),
    }
}

#[cfg(test)]
#[path = "design_cards_tests.rs"]
mod tests;
