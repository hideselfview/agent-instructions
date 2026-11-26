//! What keeps a side worktree: the open items of any project that run in it
//! or keep it (lists/kept_work.rs), and the work in it that origin/main lacks and no open item
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
use crate::side_git::SideRead;
use crate::unlanded::Held;

#[derive(Debug)]
pub struct Kept {
    /// The open items of any project that run in the worktree, each with
    /// its words, `#N "title"`.
    pub running: Vec<(Id, String)>,
    /// The open items of any project that keep the worktree, the same way.
    pub keeping: Vec<String>,
    /// The work origin/main lacks that no open item holds. An item that
    /// keeps the worktree holds all of it.
    pub unheld: Held,
}

impl Kept {
    /// Does nothing keep the worktree, so that `builds new` may hand it out
    /// once it has no claim?
    pub fn is_empty(&self) -> bool {
        self.running.is_empty() && self.keeping.is_empty() && self.unheld.is_empty()
    }

    /// May the worktree's claim go: does no item run there, and does every
    /// piece of its work that origin/main lacks have an item that holds it?
    /// An item that keeps the worktree holds its work.
    pub fn is_leavable(&self) -> bool {
        self.running.is_empty() && self.unheld.is_empty()
    }

    /// Refuses <what> (a command that removes the claim of <top>, or the
    /// worktree itself) while an item runs there, since the claim names the
    /// item's agent, or while the worktree holds work no item holds. An item
    /// that keeps the worktree holds its work, so it refuses nothing here.
    pub fn require_leavable(&self, what: &str, top: &Path) -> Result<()> {
        if let Some((first, _)) = self.running.first() {
            let id = first.0;
            let named: Vec<&str> = self
                .running
                .iter()
                .map(|(_, words)| words.as_str())
                .collect();
            fail!(
                "{what} refuses {}: the worktree still runs {}. Land the item (builds land --done {id}), stop the item (builds item stop {id} --why WHY) or hand the item to the next agent (builds item handoff {id} --brief PATH) first",
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
        item.named((every.len() > 1).then_some(lists.project.as_str()))
    };
    let running: Vec<(Id, String)> = crate::lists::running::running_in_any(every, &top)
        .into_iter()
        .map(|(lists, item)| (item.id, words(lists, item)))
        .collect();
    let keeping: Vec<String> = keeping_items(every, &top)
        .map(|(lists, item)| words(lists, item))
        .collect();
    let unheld = if keeping.is_empty() {
        let committing =
            crate::procs::commits_in(commits, &checkout.top).map_err(fail::Fail::Message)?;
        crate::unlanded::in_worktree(checkout, window_start(checkout)?, every, committing, None)?
    } else {
        Held::default()
    };
    Ok(Kept {
        running,
        keeping,
        unheld,
    })
}

/// The open item of each list in <every> that keeps the worktree <top>.
fn keeping_items<'a>(
    every: &'a [Lists],
    top: &'a str,
) -> impl Iterator<Item = (&'a Lists, &'a crate::lists::item::Item)> {
    every.iter().filter_map(move |lists| {
        crate::lists::kept_work::kept_in(lists, top).map(|item| (lists, item))
    })
}

/// Whether an eligible idle claim can go, with the other branches that
/// hold copies of its work. Each fact is read only once the facts before it
/// leave the claim eligible: no Git read is needed when an item keeps it,
/// and <known>, a read of the worktree's git state the caller already has,
/// stands in for the reads it holds. <committing> says which `git commit`s
/// run there, or why builds cannot tell.
pub fn idle_claim(
    checkout: &Checkout,
    claim: &Claim,
    every: &[Lists],
    nothing_runs: bool,
    committing: impl FnOnce() -> std::result::Result<Vec<i32>, String>,
    known: Option<&SideRead>,
) -> Result<Option<std::collections::BTreeSet<String>>> {
    if !nothing_runs || claim.is_held() || claim.is_starting() {
        return Ok(None);
    }
    let top = checkout.top.to_string_lossy();
    if keeping_items(every, &top).next().is_some() {
        return Ok(None);
    }
    let committing = committing().map_err(fail::Fail::Message)?;
    if !committing.is_empty() {
        return Ok(None);
    }
    let work = crate::unlanded::in_worktree(checkout, claim.since, every, Vec::new(), known)?;
    Ok(work.git_is_empty().then_some(work.copied_on))
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
