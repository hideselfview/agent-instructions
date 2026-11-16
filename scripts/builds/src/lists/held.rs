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
use crate::lists::design::DesignChange;
use crate::lists::design_paths::{Changes, Globs, changes_of, is_ancestor};
use crate::lists::running::running_in;
use crate::lists::store::{Loaded, Store, lists_of, project_of};
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

/// Why <worktree> may not be held in <lists>: no item runs there, or a
/// running item waits on nothing open. None when it may.
fn hold_fault(lists: &Lists, worktree: &str) -> Option<String> {
    let running = running_in(lists, worktree);
    if running.is_empty() {
        return Some(format!(
            "{worktree} runs no item, so nothing says what it waits on"
        ));
    }
    let idle: Vec<String> = running
        .iter()
        .filter(|item| !waits_on_open(lists, item))
        .map(|item| format!("{} \"{}\"", item.id.0, item.title))
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
    let Some(project) = project_of(places, checkout)? else {
        fail!(
            "{top} can't be held: no lists project holds its repo, so no item there waits on anything"
        );
    };
    let lists = Store::named(places, &project)?.load()?;
    match hold_fault(&lists, &top) {
        None => Ok(()),
        Some(fault) => fail!(
            "{fault}; a held worktree waits on an open record. Make the running item wait on one with `builds item wait N --on REF` (REF is question:N, item:N, design:N or condition:TEXT), then note it held"
        ),
    }
}

/// A claimed worktree as `builds health` read it.
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

/// One commit past origin/main that changes design files, and the open
/// design change that covers it, if one does.
struct DesignCommit<'a> {
    sha: String,
    changes: Changes,
    covered_by: Option<&'a DesignChange>,
}

/// Each commit of <worktree> past origin/main that changes design files, with
/// the open design change of its repo that covers it, approved or waiting on
/// the person's review. A change covers a commit when the change's last
/// commit is that commit or a later one, and the change's places hold every
/// place the commit changes: the person reviews each of those places drawn at
/// the change's last commit, which holds the commit's changes.
fn design_commits<'a>(
    lists: &'a Lists,
    worktree: &Worktree,
    globs: &Globs,
) -> Result<Vec<DesignCommit<'a>>> {
    let open: Vec<(&DesignChange, &String)> = lists
        .designs
        .iter()
        .filter(|design| {
            design.repo == worktree.repo
                && matches!(
                    lists.locate(design.id),
                    Some(Located {
                        standing: Standing::Open,
                        ..
                    })
                )
        })
        .filter_map(|design| design.tip.as_ref().map(|tip| (design, tip)))
        .collect();
    let mut found = Vec::new();
    for sha in &worktree.ahead {
        let changes = changes_of(&worktree.top, globs, std::slice::from_ref(sha))?;
        if !changes.is_design_change() {
            continue;
        }
        let mut covered_by = None;
        for (design, tip) in &open {
            let holds_places = changes
                .resolvable()
                .iter()
                .all(|place| design.places.contains_key(*place));
            if holds_places && is_ancestor(&worktree.top, sha, tip)? {
                covered_by = Some(*design);
                break;
            }
        }
        found.push(DesignCommit {
            sha: sha.clone(),
            changes,
            covered_by,
        });
    }
    Ok(found)
}

/// The design commits of <worktree> (`design_commits`), or None when its
/// lists name no design globs for its repo or it has no commits past
/// origin/main.
fn design_commits_of<'a>(
    lists: &'a Lists,
    worktree: &Worktree,
) -> Option<Result<Vec<DesignCommit<'a>>>> {
    let globs = Globs::of(lists, &worktree.repo);
    if worktree.ahead.is_empty() || (globs.places.is_empty() && globs.sources.is_empty()) {
        return None;
    }
    Some(design_commits(lists, worktree, &globs))
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
            let Some(Ok(commits)) = design_commits_of(lists, worktree) else {
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
    if loaded.is_err() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for worktree in worktrees {
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
        let top = worktree.top.display().to_string();
        // An app checkout runs no item, so health reports neither a hold nor
        // a claim on it for that.
        let standing = worktree.app_checkout;
        if !standing
            && worktree.claim.note_state.as_deref() == Some("held")
            && let Some(fault) = hold_fault(lists, &top)
        {
            found.push(Advice::run(
                format!(
                    "lists: {} is held, but {fault}; start its work again (builds item take or builds item handoff), then clear the hold with",
                    worktree.label
                ),
                format!("builds note {top} TEXT"),
            ));
        }
        if !standing
            && worktree.claim.note_state.as_deref() != Some("held")
            && worktree.ahead.is_empty()
            && running_in(lists, &top).is_empty()
        {
            found.push(Advice::run(
                format!(
                    "lists: {} claims {}, but no item runs there and the worktree has no commits that origin/main lacks; release it with",
                    worktree.claim.who, worktree.label
                ),
                format!("builds release {top}"),
            ));
        }
        let uncovered: Vec<DesignCommit> = match design_commits_of(lists, worktree) {
            None => continue,
            Some(Err(error)) => {
                found.push(Advice::said(format!(
                    "lists: could not read the design files {} changes: {}",
                    worktree.label,
                    error.text()
                )));
                continue;
            }
            Some(Ok(commits)) => commits
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
