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

use super::cards::{ItemWait, ReviewWait, Settler};
use crate::fail::Fail;
use crate::lists::store::Store;
use crate::review::landing::Verdict;
use crate::review::verdicts::{Reads, Worktree};

/// What the review's records and the branch's net change say of a
/// worktree's commits.
#[derive(Clone, Copy)]
enum Judgment<'a> {
    /// No group's rules cover a path the commits change.
    NotNeeded,
    /// Land takes the commits as far as the review goes.
    Takes,
    /// No review of the change is open, or a group gave no answer over a
    /// diff the review read.
    NotRun,
    /// This many findings wait for a settle.
    ToSettle(&'a [crate::review::record::Finding]),
    /// This many paths, or the commit messages, changed since the last review.
    Unreviewed(usize),
    /// This many confirmed findings are open.
    Open(usize),
}

impl<'a> Judgment<'a> {
    /// What <verdict> says of the commits, as a card shows it.
    fn of(verdict: &'a Verdict) -> Self {
        match verdict {
            Verdict::NotNeeded => Judgment::NotNeeded,
            Verdict::Takes(_) | Verdict::OverCap(_) => Judgment::Takes,
            Verdict::NoRun => Judgment::NotRun,
            Verdict::Unfinished(_, unfinished) if !unfinished.unsettled.is_empty() => {
                Judgment::ToSettle(&unfinished.unsettled)
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
    sessions: &crate::inbox::Sessions,
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
        Judgment::ToSettle(findings) => {
            let conn = store.open_existing().map_err(Fail::text)?;
            let judge = crate::review::judge::listening(&conn, sessions).map_err(Fail::text)?;
            let mut settler = if judge.is_some() {
                Settler::Judge
            } else {
                Settler::Coordinator
            };
            let mut reviewed = std::collections::BTreeSet::new();
            for finding in findings {
                if !reviewed.insert(finding.run_id) {
                    continue;
                }
                let is = |owner: &crate::ack::agent::Caller| match &judge {
                    Some(judge) => judge.is(owner),
                    None => !owner.is_dispatched(),
                };
                if crate::review::judge::owner_matches(&conn, finding, is).map_err(Fail::text)? {
                    settler = Settler::Independent;
                    break;
                }
            }
            ReviewWait::ToSettle(findings.len(), settler)
        }
        Judgment::Unreviewed(count) => ReviewWait::Unreviewed(count),
        Judgment::Open(count) => ReviewWait::Open(count),
    };
    Ok(Some(ItemWait::RulesReview(wait)))
}

#[cfg(test)]
#[path = "review_cards_tests.rs"]
mod tests;
