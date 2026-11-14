//! Free side worktrees: ones nobody holds and no work is left in. `builds new`
//! hands one out before adding another, and `builds prune` retires the ones
//! nothing has used for a day, so they don't pile up.
//!
//! A side worktree is free when it has no claim, a clean tree, and no commit
//! missing from origin/main. Landing rebases, so "missing" is judged by patch
//! (`side_git`, `git cherry`), not by hash. main and the repo's app checkout
//! are never free: the person's builds and the agents' app builds stay warm in
//! them.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::checkout::{Checkout, Root};
use crate::claim::Claim;
use crate::fail::Result;
use crate::health::places_of;
use crate::places::FREE_WORKTREE_IDLE_MINUTES;
use crate::procs::{BUILD_NAMES, Processes, build_processes};
use crate::prune::Run;
use crate::side_git::{self, SideGit};
use crate::text::note;
use crate::tool::Tool;
use crate::worktree::retire_checkout;

/// Is the side worktree <checkout> free, given what git says about it?
fn is_free(checkout: &Checkout, git: &SideGit) -> Result<bool> {
    Ok(Claim::read(&checkout.claim_file())?.is_none()
        && git.error.is_none()
        && git.uncommitted == Some(0)
        && git.ahead.is_empty())
}

/// The first free side worktree of <main>'s repo, by path; origin/main must be
/// fetched.
pub fn find_free(tool: &Tool, main: &Path) -> Result<Option<Checkout>> {
    let mut tops = crate::checkout::worktrees_of(main)?;
    tops.sort();
    for top in tops {
        let Some(checkout) = Checkout::at(&top)? else {
            continue;
        };
        if !checkout.is_side() || checkout.top != top {
            continue;
        }
        if tool
            .places
            .app_lane_of(&checkout.repo(), &checkout.name())?
            .is_some()
        {
            continue;
        }
        if is_free(&checkout, &side_git::read(&checkout.top))? {
            return Ok(Some(checkout));
        }
    }
    Ok(None)
}

/// The newest write to the worktree's own git dir, in seconds since 1970: a
/// checkout writes HEAD and the index, a released claim changes the folder.
fn git_dir_written(checkout: &Checkout) -> i64 {
    [
        checkout.git_dir.clone(),
        checkout.git_dir.join("HEAD"),
        checkout.git_dir.join("index"),
    ]
    .iter()
    .filter_map(|path| std::fs::symlink_metadata(path).ok())
    .map(|meta| meta.mtime())
    .max()
    .unwrap_or(0)
}

/// Retires each free side worktree among <roots> that nothing has used for
/// FREE_WORKTREE_IDLE_MINUTES: no claim, no write to its tree's git dir or
/// build folders, no build process there. Never main, the app checkout, side-1
/// (the repo's long-lived side worktree) or a claimed one (a landing worktree's
/// claim holds it). The worktrees that keep commits
/// missing from origin/main stay (health lists them).
pub fn retire_idle(tool: &Tool, roots: &[Root], run: Run) -> Result<()> {
    let places = places_of(roots);
    let processes = Processes::read();
    let builds = build_processes(&processes, &places, &BUILD_NAMES);
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if !checkout.is_side() || root.keeps_warm(&tool.places)? {
            continue;
        }
        if !is_free(checkout, &side_git::read(&checkout.top))? {
            continue;
        }
        // Each repo's long-lived side worktree stays, free or not.
        if checkout.name() == "side-1" {
            continue;
        }
        let idle = FREE_WORKTREE_IDLE_MINUTES;
        if tool.now - git_dir_written(checkout) < (idle * 60) as i64 {
            continue;
        }
        let Some(place) = places.iter().position(|p| p.root == root.path) else {
            continue;
        };
        if builds.iter().any(|b| b.place == place) {
            continue;
        }
        if root.path.is_dir() && tool.written_within(&tool.measure(&root.path)?, idle) {
            continue;
        }
        if run.dry() {
            note(format!(
                "would retire {} (free, unused for {idle}m)",
                checkout.top.display()
            ));
        } else {
            retire_checkout(tool, checkout)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "free_worktrees_tests.rs"]
mod tests;
