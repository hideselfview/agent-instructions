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

/// The dirty paths of <top> that none of <commits> holds exactly.
fn foreign_changes(top: &Path, commits: &[&str]) -> Result<Vec<String>> {
    let mut foreign = dirty(top)?;
    for commit in commits {
        if foreign.is_empty() {
            break;
        }
        let differs = differing(top, commit)?;
        foreign.retain(|path| differs.contains(path));
    }
    Ok(foreign.into_iter().collect())
}

/// <matching>: a commit the dirty paths may match as well, the one a halfway
/// switch was heading to when it is not origin's main today.
pub fn cmd_reset(places: &Places, repo: &str, checkout: &str, matching: Option<&str>) -> Result<()> {
    let Some(lane) = places.app_lane_of(repo, checkout)? else {
        fail!("{checkout} is not one of {repo}'s app checkouts (builds status lists them)");
    };
    let top = app_checkout_top(places, repo, checkout)?;
    let lock = lock::take_checkout(places, repo, lane, checkout)?;
    let reset = reset_under_lock(&top, matching);
    unsafe { libc::close(lock) };
    reset?;
    note(format!("{} is on {TARGET}", top.display()));
    Ok(())
}

/// The reset, with the checkout's lock held.
fn reset_under_lock(top: &Path, matching: Option<&str>) -> Result<()> {
    run_git(top, &["fetch", "-q", "origin"])?;
    let resolve = |rev: &str| run_git(top, &["rev-parse", "--verify", &format!("{rev}^{{commit}}")]).map(|id| id.trim().to_string());
    let target = resolve(TARGET)?;
    let matching = matching.map(resolve).transpose()?;
    let mut commits = vec![target.as_str(), "HEAD"];
    commits.extend(matching.as_deref());
    let foreign = foreign_changes(top, &commits)?;
    if !foreign.is_empty() {
        let also = matching.map(|rev| format!(" nor {rev}'s")).unwrap_or_default();
        fail!(
            "{} has changes that are neither {TARGET}'s ({target}) nor HEAD's{also}, so nothing was changed; find whose they are:\n  {}",
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
