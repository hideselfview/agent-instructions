//! Free side worktrees: ones nobody holds and no work is left in. `builds new`
//! hands one out before adding another, and `builds prune` retires the ones
//! nothing has used for a day, so they don't pile up.
//!
//! A side worktree is free when it has no claim and nothing keeps it
//! (kept.rs): no item of any project runs or is parked in it, its tree is
//! clean, no `git commit` runs there, and neither its HEAD nor a branch it
//! left since its last claim began holds a commit missing from origin/main.
//! `builds release` refuses to remove a claim on the same facts. Landing
//! rebases, so `side_git` compares commits by patch (`git cherry`), not by
//! hash. main and the repo's app checkout are never free: the person's
//! builds and the agents' app builds stay warm in them.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::checkout::{Checkout, Root};
use crate::claim::Claim;
use crate::fail::Result;
use crate::health::places_of;
use crate::lists::Lists;
use crate::places::FREE_WORKTREE_IDLE_MINUTES;
use crate::procs::{BUILD_NAMES, GitCommits, Processes, build_processes};
use crate::prune::Run;
use crate::text::note;
use crate::tool::Tool;
use crate::worktree::retire_checkout;

/// Is the side worktree <checkout> free, given <every> project's lists and
/// <commits>, the `git commit`s that run?
fn is_free(checkout: &Checkout, every: &[Lists], commits: &GitCommits) -> Result<bool> {
    Ok(Claim::read(&checkout.claim_file())?.is_none()
        && crate::kept::read(every, checkout, commits)?.is_empty())
}

/// The first free side worktree of <main>'s repo, by path; origin/main must be
/// fetched. Nothing here changes a worktree, so a worktree it passes over
/// stays as it was.
pub fn find_free(tool: &Tool, main: &Path) -> Result<Option<Checkout>> {
    let every = crate::lists::store::every_lists(&tool.places)?;
    let commits = crate::procs::git_commits(&Processes::read());
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
        if is_free(&checkout, &every, &commits)? {
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
/// build folders, no build process there. It never retires main, the app
/// checkout, side-1 (the repo's long-lived side worktree) or a claimed
/// worktree, and it keeps the worktrees that hold commits missing from
/// origin/main (health lists them).
pub fn retire_idle(tool: &Tool, roots: &[Root], run: Run) -> Result<()> {
    let places = places_of(roots);
    let every = crate::lists::store::every_lists(&tool.places)?;
    let processes = Processes::read();
    let commits = crate::procs::git_commits(&processes);
    let builds = build_processes(&processes, &places, &BUILD_NAMES);
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if !checkout.is_side() || root.keeps_warm(&tool.places)? {
            continue;
        }
        if !is_free(checkout, &every, &commits)? {
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
