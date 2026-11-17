//! What keeps a side worktree: the open items of any project that run or are
//! parked in it, and the work in it that origin/main lacks and no open item
//! holds (unlanded.rs). `builds release` and a landing's release refuse to
//! remove the claim of a worktree where an item runs or that holds such
//! work. `builds new` hands out, and `builds prune` and `builds retire`
//! remove, only an unclaimed worktree that nothing keeps. Each of them reads
//! it here before it changes the worktree, so they agree on what a worktree
//! holds.

use std::path::Path;

use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::fail;
use crate::fail::Result;
use crate::lists::{Id, Lists};
use crate::unlanded::Held;

#[derive(Debug)]
pub struct Kept {
    /// The open items of any project that run in the worktree, each with
    /// its words, `#N "title"`.
    pub running: Vec<(Id, String)>,
    /// The open items of any project that are parked in the worktree, the
    /// same way.
    pub parked: Vec<String>,
    /// The work origin/main lacks that no open item holds. An item parked in
    /// the worktree holds all of it.
    pub unheld: Held,
}

impl Kept {
    /// Does nothing keep the worktree, so that `builds new` may hand it out
    /// once it has no claim?
    pub fn is_empty(&self) -> bool {
        self.running.is_empty() && self.parked.is_empty() && self.unheld.is_empty()
    }

    /// May the worktree's claim go: does no item run there, and does every
    /// piece of its work that origin/main lacks have an item that holds it?
    /// An item parked there holds its work.
    pub fn is_leavable(&self) -> bool {
        self.running.is_empty() && self.unheld.is_empty()
    }

    /// Refuses <what> (a command that removes the claim of <top>, or the
    /// worktree itself) while an item runs there, since the claim names the
    /// item's agent, or while the worktree holds work no item holds. An item
    /// parked there holds its work, so it refuses nothing here.
    pub fn require_leavable(&self, what: &str, top: &Path) -> Result<()> {
        if let Some((first, _)) = self.running.first() {
            let id = first.0;
            let named: Vec<&str> = self
                .running
                .iter()
                .map(|(_, words)| words.as_str())
                .collect();
            fail!(
                "{what} refuses {}: it still runs {}. Land it (builds land --done {id}), stop it (builds item stop {id} --why WHY) or hand it to the next agent (builds item handoff {id} --brief PATH) first",
                top.display(),
                named.join("; ")
            );
        }
        if !self.unheld.is_empty() {
            fail!("{}", crate::unlanded::refusal(what, top, &self.unheld));
        }
        Ok(())
    }
}

/// What keeps <checkout>, from <every> project's lists and <commits>, the
/// `git commit`s that run. The branches the worktree left count from the
/// start of its claim, or of its last claim when it has none
/// (`window_start`).
pub fn read(
    every: &[Lists],
    checkout: &Checkout,
    commits: &crate::procs::GitCommits,
) -> Result<Kept> {
    let top = checkout.top.display().to_string();
    let words = |lists: &Lists, item: &crate::lists::item::Item| {
        if every.len() > 1 {
            format!(
                "{} \"{}\" of project {}",
                item.id, item.title, lists.project
            )
        } else {
            format!("{} \"{}\"", item.id, item.title)
        }
    };
    let running: Vec<(Id, String)> = crate::lists::running::running_in_any(every, &top)
        .into_iter()
        .map(|(lists, item)| (item.id, words(lists, item)))
        .collect();
    let parked: Vec<String> = every
        .iter()
        .filter_map(|lists| {
            crate::lists::parked::parked_in(lists, &top).map(|item| words(lists, item))
        })
        .collect();
    let unheld = if parked.is_empty() {
        let repo = checkout.repo();
        let held: Vec<String> = crate::unlanded::named_branches(every.iter())
            .into_iter()
            .filter(|(named_repo, _)| *named_repo == repo)
            .map(|(_, branch)| branch)
            .collect();
        crate::unlanded::in_worktree(checkout, window_start(checkout)?, &held, commits)?
    } else {
        Held::default()
    };
    Ok(Kept {
        running,
        parked,
        unheld,
    })
}

/// When the branches <checkout> left start to count, in seconds since 1970:
/// the start of its claim, or of its last claim when it has none. A
/// worktree that has had no claim since builds began to keep the last one
/// counts every branch its HEAD's reflog says it left.
fn window_start(checkout: &Checkout) -> Result<i64> {
    let claim = match Claim::read(&checkout.claim_file())? {
        Some(claim) => Some(claim),
        None => Claim::read(&checkout.last_claim_file())?,
    };
    Ok(claim.map_or(0, |claim| claim.since))
}
