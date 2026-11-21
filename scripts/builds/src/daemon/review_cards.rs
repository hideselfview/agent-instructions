//! Where the rules review of a worktree's commits stands, for its card: the
//! commits waiting to land are judged the way `builds land` judges them
//! (`review::verdicts`), so a card says Ready to land only when land would
//! take them. A review that runs now over the branch's newest commit shows as
//! running.
//!
//! Every open page and the recorder of the cards' states ask for each card's
//! standing each time they build it. The verdict on the commits is kept for
//! each tip (`Reads`). Whether a review runs now, and whether the judge's
//! session runs, change with no write to the store, so `standing` reads them
//! each time.

use std::path::Path;

use super::cards::{ItemWait, ReviewWait, Settler};
use crate::fail::Fail;
use crate::lists::store::Store;
use crate::review::landing::Verdict;
use crate::review::verdicts::{Reads, Worktree};

/// What the review's records and the branch's net change say of a
/// worktree's commits.
#[derive(Clone, Copy)]
enum Judgment {
    /// No group's rules cover a path the commits change.
    NotNeeded,
    /// Land takes the commits as far as the review goes.
    Takes,
    /// No review of the change is open, or a group gave no answer over a
    /// diff the review read.
    NotRun,
    /// This many findings wait for a settle.
    ToSettle(usize),
    /// This many paths, or the commit messages, changed since the last review.
    Unreviewed(usize),
    /// This many confirmed findings are open.
    Open(usize),
}

impl Judgment {
    /// What <verdict> says of the commits, as a card shows it.
    fn of(verdict: &Verdict) -> Judgment {
        match verdict {
            Verdict::NotNeeded => Judgment::NotNeeded,
            Verdict::Takes(_) | Verdict::OverCap(_) => Judgment::Takes,
            Verdict::NoRun => Judgment::NotRun,
            Verdict::Unfinished(_, unfinished) if !unfinished.unsettled.is_empty() => {
                Judgment::ToSettle(unfinished.unsettled.len())
            }
            Verdict::Unfinished(_, unfinished) if !unfinished.unanswered.is_empty() => {
                Judgment::NotRun
            }
            Verdict::Unfinished(_, unfinished) if !unfinished.unreviewed.is_empty() => {
                Judgment::Unreviewed(unfinished.unreviewed.len())
            }
            Verdict::Unfinished(_, unfinished) => Judgment::Open(unfinished.open.len()),
        }
    }
}

/// Where the review of <worktree>'s commits stands, for a worktree
/// whose project's store is <store>, at version <version> as the daemon
/// last read it; None when land would take them as far as the review
/// goes, or when they need no review. Findings to settle wait on the
/// judge while one listens, which Claude Code's running sessions in
/// <sessions> say. Fails, saying why, when git cannot read the commits
/// or builds cannot open or read the store or the sessions.
pub fn standing(
    reads: &Reads,
    store: &Store,
    version: u64,
    sessions: &Path,
    worktree: &Worktree,
) -> Result<Option<ItemWait>, String> {
    let Some(newest) = worktree.git.ahead.first() else {
        return Ok(None);
    };
    let verdict = reads.verdict_at(store, version, worktree)?;
    let wait = match Judgment::of(&verdict) {
        Judgment::NotNeeded => return Ok(None),
        _ if crate::review::is_running(&store.dir, &newest.full) => ReviewWait::Running,
        Judgment::Takes => return Ok(None),
        Judgment::NotRun => ReviewWait::NotRun,
        Judgment::ToSettle(count) => {
            let conn = store.open_existing().map_err(Fail::text)?;
            let settler =
                match crate::review::judge::listening(&conn, sessions).map_err(Fail::text)? {
                    Some(_) => Settler::Judge,
                    None => Settler::Coordinator,
                };
            ReviewWait::ToSettle(count, settler)
        }
        Judgment::Unreviewed(count) => ReviewWait::Unreviewed(count),
        Judgment::Open(count) => ReviewWait::Open(count),
    };
    Ok(Some(ItemWait::RulesReview(wait)))
}

#[cfg(test)]
#[path = "review_cards_tests.rs"]
mod tests;
