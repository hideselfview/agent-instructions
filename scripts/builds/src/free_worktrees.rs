//! Free side worktrees: ones nobody holds and no work is left in. `builds new`
//! hands one out before adding another, and `builds health` and `builds prune`
//! retire them, so they don't pile up. A new worktree costs seconds, because
//! `builds new` fills its host `target` with a clone of a warm checkout's
//! (warm.rs), so a free worktree keeps nothing worth a wait.
//!
//! A side worktree is free when it has no claim and nothing keeps it
//! (kept.rs): no item of any project runs or is kept in it, its tree is
//! clean, no `git commit` runs there, and neither its HEAD nor a branch it
//! left since its last claim began holds a commit missing from origin/main.
//! `builds release` refuses to remove a claim on the same facts. Landing
//! rebases, so `side_git` compares commits by patch (`git cherry`), not by
//! hash. main and the repo's app checkout are never free: the person's
//! builds and the agents' app builds stay warm in them.

use std::path::Path;

use crate::checkout::{Checkout, Root};
use crate::claim::Claim;
use crate::fail::Result;
use crate::health::places_of;
use crate::lists::Lists;
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
    let commits = crate::procs::git_commits(&Processes::read()?);
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

/// Retires each free side worktree among <roots> that no build process
/// uses: no claim, nothing that keeps it (kept.rs). It never retires main, the
/// app checkout, side-1 (the repo's long-lived side worktree) or a claimed
/// worktree, and it keeps the worktrees that hold commits missing from
/// origin/main (health lists them). It holds the picking lock of the repo
/// while it checks and removes a worktree, so `builds new` never claims one
/// that is going.
pub fn retire_free(tool: &Tool, roots: &[Root], run: Run) -> Result<()> {
    let places = places_of(roots);
    let every = crate::lists::store::every_lists(&tool.places)?;
    let processes = Processes::read()?;
    let commits = crate::procs::git_commits(&processes);
    let builds = build_processes(&processes, &places, &BUILD_NAMES);
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if !checkout.is_side() || root.keeps_warm(&tool.places)? {
            continue;
        }
        // Each repo's long-lived side worktree stays, free or not.
        if checkout.name() == "side-1" {
            continue;
        }
        let Some(place) = places.iter().position(|p| p.root == root.path) else {
            continue;
        };
        if builds.iter().any(|b| b.place == place) {
            continue;
        }
        let _picking = crate::worktree::lock_picking(&checkout.main)?;
        if !is_free(checkout, &every, &commits)? {
            continue;
        }
        if run.dry() {
            note(format!("would retire {} (free)", checkout.top.display()));
        } else {
            retire_checkout(tool, checkout)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "free_worktrees_tests.rs"]
mod tests;
