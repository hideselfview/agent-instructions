//! Held work: a worktree kept on purpose with no agent running must wait on
//! something that can end, or no list would ever restart it. `builds note
//! --state held` asks `require_hold_allowed`; `builds health` asks
//! `findings` for the holds whose wait closed and for design changes in a
//! branch that no recorded design change covers.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::daemon::review_cards::Reads;
use crate::fail;
use crate::fail::Result;
use crate::health::Advice;
use crate::lists::design::{DesignChange, DesignCommit};
use crate::lists::design_reads::DesignReads;
use crate::lists::running::{SpanKind, end_span, open_span, running_in, running_in_any};
use crate::lists::store::{Loaded, Store, lists_of};
use crate::lists::{Id, Item, Lists, Located, Stamp, Standing, Wait};
use crate::places::Places;
use crate::side_git::SideRead;

/// Does <wait> still hold something: a condition, or an open question, item
/// or design change? A wait on a record that closed without meeting it
/// (dropped, deferred) holds nothing either.
pub fn wait_open(lists: &Lists, wait: &Wait) -> bool {
    let id = match wait {
        Wait::Condition { .. } => return true,
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
            "{fault}; a held worktree waits on an open record. Make the running item wait on one with `builds item wait N --on REF` (REF is question:N, item:N, design:N or condition:TEXT with --until agent|coordinator|ci; work that waits on the person waits on a your-step question), then note it held"
        ),
    }
}

/// Starts (when <held>) or ends at <stamp> the `held` span of each item of
/// every project that runs in <checkout>, and writes <noted>, the claim with
/// its new note, as the worktree's claim. The spans change in every store
/// or in none (`store::write_each`). record_hold writes the new claim to a
/// file beside the claim file before the stores commit, and moves it over
/// the claim file only after every store has committed, so a refused write
/// leaves the claim file as it was.
pub fn record_hold(
    places: &Places,
    checkout: &Checkout,
    held: bool,
    stamp: &Stamp,
    noted: &Claim,
) -> Result<()> {
    let top = checkout.top.display().to_string();
    let every = crate::lists::store::every_lists(places)?;
    let running: Vec<(&str, Vec<Id>)> = every
        .iter()
        .map(|lists| {
            let ids = running_in(lists, &top).iter().map(|item| item.id).collect();
            (lists.project.as_str(), ids)
        })
        .filter(|(_, ids): &(&str, Vec<Id>)| !ids.is_empty())
        .collect();
    let writes = running
        .into_iter()
        .map(|(project, ids)| Ok((Store::named(places, project)?, ids)))
        .collect::<Result<Vec<_>>>()?;
    let staged = noted.stage(&checkout.claim_file())?;
    let words = format!("note {top} {}", if held { "held" } else { "not held" });
    let spans = |db: &rusqlite::Connection, ids: &[Id]| -> Result<()> {
        for id in ids {
            if held {
                open_span(db, *id, SpanKind::Held, stamp.at)?;
            } else {
                end_span(db, *id, SpanKind::Held, stamp.at)?;
            }
        }
        Ok(())
    };
    if let Err(refused) =
        crate::lists::store::write_each(&writes, stamp, &words, |db, ids| spans(db, ids))
    {
        return Err(match staged.discard() {
            Ok(()) => refused,
            Err(removal) => crate::fail::Fail::Message(format!(
                "{}; removing the staged claim then failed too: {}",
                refused.text(),
                removal.text()
            )),
        });
    }
    staged.put()
}

/// A claimed worktree as `builds health` read it.
#[derive(Debug)]
pub struct Worktree {
    pub label: String,
    pub top: PathBuf,
    pub repo: String,
    /// The branch HEAD is on; None when HEAD is detached.
    pub branch: Option<String>,
    pub claim: Claim,
    /// The project the claim names (`builds claim --project`), or None when
    /// the claim names none.
    pub project: Option<String>,
    /// Is it one of the repo's recorded app checkouts?
    pub app_checkout: bool,
    /// What git said of it, or what git said when it could not read it.
    pub git: std::result::Result<SideRead, String>,
    /// The pids of the `git commit`s that run in it, or why builds cannot
    /// tell.
    pub committing: std::result::Result<Vec<i32>, String>,
}

impl Worktree {
    /// The claimed worktree <checkout>, labelled <label>, with its <claim>,
    /// what git says of it (<git>) and the `git commit`s that run
    /// (<commits>).
    pub fn read(
        places: &Places,
        label: String,
        checkout: &Checkout,
        claim: &Claim,
        branch: Option<String>,
        git: &std::result::Result<SideRead, String>,
        commits: &crate::procs::GitCommits,
    ) -> Result<Worktree> {
        Ok(Worktree {
            label,
            top: checkout.top.clone(),
            repo: checkout.repo(),
            branch,
            claim: claim.clone(),
            project: checkout.claimed_project()?,
            app_checkout: places
                .app_lane_of(&checkout.repo(), &checkout.name())?
                .is_some(),
            git: git.clone(),
            committing: crate::procs::commits_in(commits, &checkout.top),
        })
    }

    /// Is its claim one health releases, and says to release: not an app
    /// checkout's, not held (`builds note --state held`), not waiting for its
    /// item (`Note::Starting`), no item runs there (<nothing_runs>), and the
    /// worktree has no commits that origin/main lacks, no files not committed
    /// and no `git commit` running? `builds health` releases such a claim
    /// itself (health_fix.rs), and lists the ones it could not.
    pub fn claim_is_idle(&self, nothing_runs: bool) -> bool {
        !self.app_checkout
            && !self.claim.is_held()
            && !self.claim.is_starting()
            && self
                .git
                .as_ref()
                .is_ok_and(|git| git.ahead.is_empty() && git.uncommitted == 0)
            && self.committing.as_ref().is_ok_and(|pids| pids.is_empty())
            && nothing_runs
    }

    /// What blocks its branch from landing, read from the review records
    /// (`review::idle::landing`), or why builds cannot tell: None for a held
    /// worktree, an app checkout, one with no commits that origin/main lacks,
    /// and one whose lists `findings` already names as unreadable.
    pub fn landing(
        &self,
        places: &Places,
        loaded: &std::result::Result<Vec<Loaded>, String>,
        reviews: &Reads,
    ) -> Option<std::result::Result<crate::review::idle::Landing, String>> {
        if self.app_checkout || self.claim.is_held() {
            return None;
        }
        let git = self.git.as_ref().ok()?;
        if git.ahead.is_empty() {
            return None;
        }
        let lists = self.lists(loaded).ok()??;
        let reviewed = crate::daemon::review_cards::Worktree {
            repo: &self.repo,
            top: &self.top,
            branch: self.branch.as_deref(),
            git,
        };
        Some(reviews.verdict_of(places, lists, &reviewed).map(|verdict| {
            crate::review::idle::landing(&verdict, lists, &self.top, &git.ahead_oldest_first())
        }))
    }

    /// The lists of the project its work is in (`store::lists_of`).
    fn lists<'a>(
        &self,
        loaded: &'a std::result::Result<Vec<Loaded>, String>,
    ) -> std::result::Result<Option<&'a Lists>, String> {
        lists_of(loaded, &self.top, &self.repo, self.project.as_deref())
    }
}

/// The labels of the worktrees whose commits an open design change covers
/// that waits on the person's approval. Such a worktree waits on the
/// person's review once the rules review takes its commits (decision #362),
/// and then it is not quiet; once the person approves, the change waits on
/// landing and the worktree is quiet again. <reads> holds the design commits
/// read so far.
pub fn awaiting_approval(
    loaded: &std::result::Result<Vec<Loaded>, String>,
    worktrees: &[Worktree],
    reads: &DesignReads,
) -> Vec<String> {
    worktrees
        .iter()
        .filter(|worktree| {
            // `findings` names a worktree whose lists health can't choose.
            let Ok(Some(lists)) = worktree.lists(loaded) else {
                return false;
            };
            // `findings` names a worktree whose commits or design files
            // health can't read.
            let Ok(git) = &worktree.git else {
                return false;
            };
            let Ok(commits) = reads.of(lists, &worktree.repo, &worktree.top, git) else {
                return false;
            };
            commits.iter().any(|commit| {
                commit
                    .covering(lists)
                    .is_some_and(DesignChange::awaits_approval)
            })
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
/// builds can't list the stores. <reads> holds the design commits read so
/// far.
pub fn findings(
    loaded: &std::result::Result<Vec<Loaded>, String>,
    worktrees: &[Worktree],
    reads: &DesignReads,
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
            && worktree.claim.is_held()
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
        // Release refuses a worktree with commits, files or a running git
        // commit that nothing else holds, so health says to release only one
        // with none of them.
        if worktree.claim_is_idle(running.is_empty()) {
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
        let git = match &worktree.git {
            Ok(git) => git,
            Err(error) => {
                found.push(Advice::said(format!(
                    "lists: could not read the commits of {}, so health can't tell whether a design change covers them: {error}",
                    worktree.label
                )));
                continue;
            }
        };
        let commits = match reads.of(lists, &worktree.repo, &worktree.top, git) {
            Err(error) => {
                found.push(Advice::said(format!(
                    "lists: could not read the design files {} changes: {}",
                    worktree.label,
                    error.text()
                )));
                continue;
            }
            Ok(commits) => commits,
        };
        let uncovered: Vec<&DesignCommit> = commits
            .iter()
            .filter(|commit| commit.covered_by.is_none())
            .collect();
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
