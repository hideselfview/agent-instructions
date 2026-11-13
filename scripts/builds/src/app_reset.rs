//! `builds app-checkout reset <repo> <checkout>`: puts an app checkout that a
//! checkout left halfway on origin's main, under that checkout's own lock.
//!
//! A switch killed by SIGKILL (app_config.rs `switch_to` holds out against
//! every other signal) leaves the new commit's files under the old HEAD, which
//! every build refuses as a dirty tree. The reset takes the lock, waiting for
//! the build that holds it, and refuses unless every dirty path is exactly
//! what the target commit or HEAD holds there (content and mode, absent
//! included), so it never throws away work: it names the paths that are
//! neither. Then it checks the target out over the tree.
//!
//! "Exactly what a commit holds" is asked of git: a scratch index filled from
//! the commit, refreshed, tells which paths of the working tree differ from it
//! (`diff-files`) and which are not in it (`ls-files --others`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::app_config::{app_checkout_top, run_git, switch_to};
use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::Result;
use crate::lock;
use crate::places::Places;
use crate::text::note;

/// What the checkout is put on.
const TARGET: &str = "origin/main";

/// A scratch index inside the checkout's git dir, removed when dropped.
struct ScratchIndex(PathBuf);

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The paths of the working tree in <top> that differ from <commit>'s tree:
/// changed, deleted, or not in it.
fn differing(top: &Path, commit: &str) -> Result<BTreeSet<String>> {
    let git_dir = run_git(top, &["rev-parse", "--absolute-git-dir"])?;
    let index = ScratchIndex(Path::new(git_dir.trim()).join(format!("builds-reset-index-{}", std::process::id())));
    let with_index = || {
        let mut command = git(top);
        command.env("GIT_INDEX_FILE", &index.0);
        command
    };
    git_text(with_index().args(["read-tree", commit]), &format!("read {commit}'s tree"))?;
    // Differing files make --refresh exit 1 even with -q; the answer is in
    // diff-files below.
    let _ = with_index().args(["update-index", "-q", "--refresh"]).output();
    let mut paths = BTreeSet::new();
    for args in [&["diff-files", "--name-only", "-z"][..], &["ls-files", "--others", "--exclude-standard", "-z"][..]] {
        let listed = git_text(with_index().args(args), &format!("list what differs from {commit}"))?;
        paths.extend(listed.split('\0').filter(|path| !path.is_empty()).map(str::to_string));
    }
    Ok(paths)
}

/// The paths `git status` calls dirty in <top>: staged, changed and untracked
/// alike, each file named.
fn dirty(top: &Path) -> Result<BTreeSet<String>> {
    let status = run_git(top, &["status", "--porcelain", "-z", "-uall", "--no-renames"])?;
    Ok(status.split('\0').filter(|entry| entry.len() > 3).map(|entry| entry[3..].to_string()).collect())
}

/// The dirty paths of <top> that neither <target> nor HEAD holds exactly.
fn foreign_changes(top: &Path, target: &str) -> Result<Vec<String>> {
    let dirty = dirty(top)?;
    if dirty.is_empty() {
        return Ok(Vec::new());
    }
    let (from_target, from_head) = (differing(top, target)?, differing(top, "HEAD")?);
    Ok(dirty.into_iter().filter(|path| from_target.contains(path) && from_head.contains(path)).collect())
}

pub fn cmd_reset(places: &Places, repo: &str, checkout: &str) -> Result<()> {
    let Some(lane) = places.app_lane_of(repo, checkout)? else {
        fail!("{checkout} is not one of {repo}'s app checkouts (builds status lists them)");
    };
    let top = app_checkout_top(places, repo, checkout)?;
    let lock = lock::take_checkout(places, repo, lane, checkout)?;
    let reset = reset_under_lock(&top);
    unsafe { libc::close(lock) };
    reset?;
    note(format!("{} is on {TARGET}", top.display()));
    Ok(())
}

/// The reset, with the checkout's lock held.
fn reset_under_lock(top: &Path) -> Result<()> {
    run_git(top, &["fetch", "-q", "origin"])?;
    let target = run_git(top, &["rev-parse", "--verify", &format!("{TARGET}^{{commit}}")])?.trim().to_string();
    let foreign = foreign_changes(top, &target)?;
    if !foreign.is_empty() {
        fail!(
            "{} has changes that are neither {TARGET}'s ({target}) nor HEAD's, so nothing was changed; find whose they are:\n  {}",
            top.display(),
            foreign.join("\n  ")
        );
    }
    switch_to(top, &target, true)?;
    let left = run_git(top, &["status", "--porcelain"])?;
    if !left.trim().is_empty() {
        fail!("{} is still dirty after the reset:\n{left}", top.display());
    }
    Ok(())
}

#[cfg(test)]
#[path = "app_reset_tests.rs"]
mod tests;
