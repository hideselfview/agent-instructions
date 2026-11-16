//! Held work: a worktree kept on purpose with no agent running must wait on
//! something that can end, or no list would ever restart it. `builds note
//! --state held` asks `require_hold_allowed`; `builds health` asks
//! `findings` for the holds whose wait closed and for design changes in a
//! branch that no recorded design change covers.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::fail;
use crate::fail::Result;
use crate::health::Advice;
use crate::lists::design::{DesignChange, DesignCommit, design_commits};
use crate::lists::running::running_in_any;
use crate::lists::store::{Loaded, lists_of};
use crate::lists::{Item, Lists, Located, Standing, Wait};
use crate::places::Places;

/// Does <wait> still hold something: a condition, or an open question, item
/// or design change? A wait on a record that closed without meeting it
/// (dropped, deferred) holds nothing either.
pub fn wait_open(lists: &Lists, wait: &Wait) -> bool {
    let id = match wait {
        Wait::Condition(_) => return true,
        Wait::Item(id) | Wait::Question(id) | Wait::Design(id) => *id,
    };
    matches!(
        lists.locate(id),
        Some(Located {
            standing: Standing::Open,
            ..
        })
    )
}

pub fn waits_on_open(lists: &Lists, item: &Item) -> bool {
    item.waits.iter().any(|wait| wait_open(lists, wait))
}

/// Why <worktree> may not be held, given the items of every project that run
/// there (<running>, each with its lists): no item runs there, or a running
/// item waits on nothing open. None when it may.
fn hold_fault(running: &[(&Lists, &Item)], worktree: &str) -> Option<String> {
    if running.is_empty() {
        return Some(format!(
            "{worktree} runs no item, so nothing says what it waits on"
        ));
    }
    let idle: Vec<String> = running
        .iter()
        .filter(|(lists, item)| !waits_on_open(lists, item))
        .map(|(_, item)| format!("{} \"{}\"", item.id.0, item.title))
        .collect();
    if idle.is_empty() {
        None
    } else {
        Some(format!(
            "{worktree} runs {}, which waits on nothing open",
            idle.join("; ")
        ))
    }
}

/// Refuses `builds note --state held` unless every item running in the
/// worktree waits on an open question, item, design change or condition.
/// Never refuses it for an app checkout (the recorded app-checkout list),
/// which runs no item.
pub fn require_hold_allowed(places: &Places, checkout: &Checkout) -> Result<()> {
    let top = checkout.top.display().to_string();
    if checkout.is_side()
        && places
            .app_lane_of(&checkout.repo(), &checkout.name())?
            .is_some()
    {
        return Ok(());
    }
    let every = crate::lists::store::every_lists(places)?;
    match hold_fault(&running_in_any(&every, &top), &top) {
        None => Ok(()),
        Some(fault) => fail!(
            "{fault}; a held worktree waits on an open record. Make the running item wait on one with `builds item wait N --on REF` (REF is question:N, item:N, design:N or condition:TEXT), then note it held"
        ),
    }
}

/// A claimed worktree as `builds health` read it.
#[derive(Debug)]
pub struct Worktree {
    pub label: String,
    pub top: PathBuf,
    pub repo: String,
    pub claim: Claim,
    /// The project the claim names (`builds claim --project`), or None when
    /// the claim names none.
    pub project: Option<String>,
    /// Is it one of the repo's recorded app checkouts?
    pub app_checkout: bool,
    /// The commits origin/main has no copy of, oldest first.
    pub ahead: Vec<String>,
    /// How many files are not committed; None when git could not tell.
    pub uncommitted: Option<usize>,
    /// The pids of the `git commit`s that run in it, or why builds cannot
    /// tell.
    pub committing: std::result::Result<Vec<i32>, String>,
}

impl Worktree {
    /// The lists of the project its work is in (`store::lists_of`).
    fn lists<'a>(
        &self,
        loaded: &'a std::result::Result<Vec<Loaded>, String>,
    ) -> std::result::Result<Option<&'a Lists>, String> {
        lists_of(loaded, &self.top, &self.repo, self.project.as_deref())
    }
}

/// The labels of the worktrees whose commits an open design change covers
/// and that waits on the person's review: they wait on the person, so they
/// are not quiet. Once the person approves, the change waits on landing and
/// the worktree is quiet again.
pub fn awaiting_review(
    loaded: &std::result::Result<Vec<Loaded>, String>,
    worktrees: &[Worktree],
) -> Vec<String> {
    worktrees
        .iter()
        .filter(|worktree| {
            // `findings` names a worktree whose lists health can't choose.
            let Ok(Some(lists)) = worktree.lists(loaded) else {
                return false;
            };
            // `findings` names a worktree whose design files health can't read.
            let Ok(commits) = design_commits(lists, &worktree.repo, &worktree.top, &worktree.ahead)
            else {
                return false;
            };
            commits
                .iter()
                .any(|commit| commit.covered_by.is_some_and(DesignChange::waits_on_person))
        })
        .map(|worktree| worktree.label.clone())
        .collect()
}

/// What `builds health` says about held work, one line each: a claimed
/// worktree whose items all closed and that has nothing past origin/main, a
/// held worktree
/// whose item waits on nothing open (the record it waited on closed, or
/// nothing runs there), and a branch whose commits change design files that
/// no open design change covers. It also names a worktree whose lists it
/// can't choose: its claim names a project that does not hold its repo,
/// names none of the two that do, or names a project whose store fails its
/// check. `running::findings` names the stores when builds can't list them
/// and each store that fails its check, so `findings` says nothing more when
/// builds can't list the stores.
pub fn findings(
    loaded: &std::result::Result<Vec<Loaded>, String>,
    worktrees: &[Worktree],
) -> Vec<Advice> {
    let Ok(every) = loaded else {
        return Vec::new();
    };
    // Health reads the items of every project, whatever project the claim
    // names: in a repo two projects hold, either project's items run in its
    // worktrees. `running::findings` names a store that fails its check.
    let every: Vec<Lists> = every
        .iter()
        .filter_map(|project| project.lists.clone().ok())
        .collect();
    let mut found = Vec::new();
    for worktree in worktrees {
        let top = worktree.top.display().to_string();
        let running = running_in_any(&every, &top);
        // An app checkout runs no item, so health reports neither a hold nor
        // a claim on it for that.
        let standing = worktree.app_checkout;
        if !standing
            && worktree.claim.note_state.as_deref() == Some("held")
            && let Some(fault) = hold_fault(&running, &top)
        {
            found.push(Advice::run(
                format!(
                    "lists: {} is held, but {fault}; start its work again (builds item take or builds item handoff), then clear the hold with",
                    worktree.label
                ),
                format!("builds note {top} TEXT"),
            ));
        }
        // Release refuses a worktree with commits, files or a commit in
        // flight that nothing else holds, so health says to release only one
        // with none of them.
        if !standing
            && worktree.claim.note_state.as_deref() != Some("held")
            && worktree.ahead.is_empty()
            && worktree.uncommitted == Some(0)
            && worktree
                .committing
                .as_ref()
                .is_ok_and(|pids| pids.is_empty())
            && running.is_empty()
        {
            found.push(Advice::run(
                format!(
                    "lists: {} claims {}, but no item runs there and the worktree has no commits that origin/main lacks, no files not committed and no git commit running; release it with",
                    worktree.claim.who, worktree.label
                ),
                format!("builds release {top}"),
            ));
        }
        let lists = match worktree.lists(loaded) {
            Ok(Some(lists)) => lists,
            Ok(None) => continue,
            Err(fault) => {
                found.push(Advice::said(format!(
                    "lists: health can't choose the lists of {}: {fault}",
                    worktree.label
                )));
                continue;
            }
        };
        let uncovered: Vec<DesignCommit> =
            match design_commits(lists, &worktree.repo, &worktree.top, &worktree.ahead) {
                Err(error) => {
                    found.push(Advice::said(format!(
                        "lists: could not read the design files {} changes: {}",
                        worktree.label,
                        error.text()
                    )));
                    continue;
                }
                Ok(commits) => commits
                    .into_iter()
                    .filter(|commit| commit.covered_by.is_none())
                    .collect(),
            };
        if uncovered.is_empty() {
            continue;
        }
        let named: BTreeSet<&str> = uncovered
            .iter()
            .flat_map(|commit| {
                let changes = &commit.changes;
                changes
                    .resolvable()
                    .into_iter()
                    .chain(&changes.sources)
                    .chain(&changes.shared)
                    .chain(&changes.deleted)
                    .map(String::as_str)
            })
            .collect();
        let shas: Vec<&str> = uncovered
            .iter()
            .map(|commit| &commit.sha[..commit.sha.len().min(10)])
            .collect();
        found.push(Advice::run(
            format!(
                "lists: {} has commits ({}) that change design files ({}) and no recorded design change covers them; record one with",
                worktree.label,
                shas.join(", "),
                named.into_iter().collect::<Vec<_>>().join(", ")
            ),
            "builds design record".to_string(),
        ));
    }
    found
}

#[cfg(test)]
#[path = "held_tests.rs"]
mod tests;
