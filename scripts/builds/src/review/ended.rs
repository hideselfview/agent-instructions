//! Review records with nothing left to land.
//!
//! A review landing stays open until its change ends (record.sql). `builds
//! land` ends it as landed when the push leaves the branch nothing to land.
//! Work also leaves a branch without that call: someone lands it another way,
//! or deletes the branch. `close` reads each open landing of a project against
//! the repo's clone on this Mac, which holds the branch of every worktree, and
//! ends the ones that have nothing left, so that health and the dashboard list
//! only the reviews that still hold work:
//!
//!   - landed, when every commit the branch holds is on origin/main by patch
//!     (`side_git::waiting_to_land`), or the branch is gone and the commit
//!     the last review read is on origin/main. The landing takes the last
//!     review's run as the run that covered the commits.
//!   - abandoned, when the branch is gone, no ref and no detached worktree
//!     holds the commit the last review read, no open item names the branch,
//!     and origin/main lacks the commit. The landing keeps its runs, findings
//!     and settles, and counts as landed nowhere.
//!
//! A landing whose branch still holds commits to land, or that an item keeps,
//! stays open. `builds health` runs `close` (health_fix.rs).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::checkout::{branch_exists, file_name, worktree_list};
use crate::fail::{Fail, Result};
use crate::lists::Stamp;
use crate::lists::store::Store;
use crate::review::record::{self, LandingRow};
use crate::side_git::{ask, waiting_to_land};
use boards::{git, git_text};

/// How an open landing ends.
#[derive(Debug, PartialEq, Eq)]
enum End {
    /// Every commit is on origin/main.
    Landed,
    /// Nothing holds the commits, and origin/main lacks them.
    Abandoned(String),
}

/// Ends each open landing of <store> whose change has nothing left to land,
/// as the module says. <mains> are the repos' main checkouts, and <named> the
/// repo and branch of each place an open item keeps. Returns one line for
/// each landing it ended and each landing it could not read.
pub fn close(
    store: &Store,
    stamp: &Stamp,
    mains: &[PathBuf],
    named: &BTreeSet<(String, String)>,
) -> Result<Vec<String>> {
    let (open, runs) = {
        let db = store.open_existing()?;
        let open: Vec<LandingRow> = record::all_landings(&db)?
            .into_iter()
            .filter(LandingRow::is_open)
            .collect();
        let mut runs = Vec::new();
        for landing in &open {
            runs.push(last_review(&db, landing)?);
        }
        (open, runs)
    };
    let mut lines = Vec::new();
    for (landing, run) in open.into_iter().zip(runs) {
        let Some(branch) = landing.key.strip_prefix("branch:") else {
            continue;
        };
        let words = format!(
            "review landing {} ({}, {})",
            landing.id, landing.repo, landing.key
        );
        let Some(main) = mains.iter().find(|main| file_name(main) == landing.repo) else {
            lines.push(format!(
                "could not judge {words}: this Mac has no main checkout of repo {}",
                landing.repo
            ));
            continue;
        };
        let Some(run) = run else {
            lines.push(format!(
                "could not judge {words}: it has no last review run to read"
            ));
            continue;
        };
        let ended = match end_of(
            main,
            branch,
            &run.head,
            named.contains(&(landing.repo.clone(), branch.to_string())),
        ) {
            Ok(Some(end)) => end,
            Ok(None) => continue,
            Err(error) => {
                lines.push(format!("could not judge {words}: {}", error.text()));
                continue;
            }
        };
        let written = store.write(
            stamp,
            &format!("review ended {}", landing.id),
            |db| match &ended {
                End::Landed => record::record_landed(db, landing.id, run.id, stamp.at),
                End::Abandoned(why) => record::record_abandoned(db, landing.id, why, stamp.at),
            },
        );
        lines.push(match (written, ended) {
            (Ok(()), End::Landed) => {
                format!("closed {words} as landed: its commits are all on origin/main")
            }
            (Ok(()), End::Abandoned(why)) => format!("closed {words} as abandoned: {why}"),
            (Err(error), _) => format!("could not close {words}: {}", error.text()),
        });
    }
    Ok(lines)
}

/// The run of <landing>'s last review, else its latest review run.
fn last_review(db: &rusqlite::Connection, landing: &LandingRow) -> Result<Option<record::RunRow>> {
    let runs = record::runs_of_landing(db, landing.id)?;
    Ok(match landing.reviewed_run {
        Some(reviewed) => runs.into_iter().find(|run| run.id == reviewed),
        None => runs.into_iter().rev().find(|run| run.kind == "review"),
    })
}

/// How the change of branch <branch> ends in the repo at <main>, whose last
/// review read <head>, or None when it still holds work. <kept> says an open
/// item names the branch.
fn end_of(main: &Path, branch: &str, head: &str, kept: bool) -> Result<Option<End>> {
    let unlanded = |rev: &str| -> Result<bool> {
        Ok(!waiting_to_land(main, rev)
            .map_err(Fail::Message)?
            .is_empty())
    };
    if branch_exists(main, branch)? {
        return Ok((!unlanded(&format!("refs/heads/{branch}"))?).then_some(End::Landed));
    }
    if kept {
        return Ok(None);
    }
    // `cat-file -e` exits 1 for a commit the repo lacks and fails otherwise,
    // and a failure must not read as a missing commit.
    let known = ask(main, &["cat-file", "-e", &format!("{head}^{{commit}}")])
        .map_err(Fail::Message)?
        .is_some();
    if known {
        if !unlanded(head)? {
            return Ok(Some(End::Landed));
        }
        if is_held(main, head)? {
            return Ok(None);
        }
    }
    Ok(Some(End::Abandoned(format!(
        "branch {branch} is gone from {}, nothing holds the commits the last review read, and no item keeps them",
        file_name(main)
    ))))
}

/// Whether a branch, remote branch or detached worktree holds <commit>.
fn is_held(main: &Path, commit: &str) -> Result<bool> {
    let refs = git_text(
        git(main).args([
            "for-each-ref",
            "--contains",
            commit,
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ]),
        "list the refs that hold a commit",
    )?;
    if !refs.trim().is_empty() {
        return Ok(true);
    }
    for listed in worktree_list(main)? {
        if listed.branch.is_none()
            && ask(
                &listed.top,
                &["merge-base", "--is-ancestor", commit, "HEAD"],
            )
            .map_err(Fail::Message)?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
#[path = "ended_tests.rs"]
mod tests;
