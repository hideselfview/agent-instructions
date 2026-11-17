//! `builds app-checkout reset <repo> <checkout>`: puts an app checkout that a
//! checkout left halfway on origin's main, under that checkout's own lock. The
//! checkout is one of this Mac's side worktrees or a checkout on a host
//! (app_remote.rs), whose lock is on this Mac too and whose git runs there
//! over ssh.
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
//!
//! A host's clone gets its commits only from this Mac: the reset reads
//! origin's main (and the commit `--matching` names) here and sends each to
//! the host's clone as a job's ref, which it deletes when it ends.

use std::collections::BTreeSet;
use std::path::Path;

use crate::app_config::{run_git, switch_to};
use crate::app_remote::local_source;
use crate::app_site::Site;
use crate::fail;
use crate::fail::{Context, Result};
use crate::host::{Ssh, sh_quote};
use crate::lock;
use crate::places::Places;
use crate::side_git::{UNCOMMITTED_STATUS, paths_in_status};
use crate::text::note;

/// What the checkout is put on.
const TARGET: &str = "origin/main";

/// The full ids of <revs>, read from origin as fetched now. A checkout on a
/// host gets each one in its clone as a job's ref, whose id goes in <sent>.
fn bring(
    site: &mut Site,
    places: &Places,
    repo: &str,
    revs: &[&str],
    sent: &mut Vec<String>,
) -> Result<Vec<String>> {
    let source = match site {
        Site::Local(top) => top.clone(),
        Site::Remote(_) => local_source(places, repo)?,
    };
    run_git(&source, &["fetch", "-q", "origin"])?;
    let mut commits = Vec::new();
    for rev in revs {
        let commit = run_git(
            &source,
            &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
        )?
        .trim()
        .to_string();
        if let Site::Remote(remote) = site {
            sent.push(remote.send(&source, &commit)?);
        }
        commits.push(commit);
    }
    Ok(commits)
}

/// Deletes the job refs <sent> that `bring` made in a host's clone.
fn forget(site: &Site, sent: &[String]) -> Result<()> {
    if let Site::Remote(remote) = site {
        for id in sent {
            remote.unsend(id)?;
        }
    }
    Ok(())
}

/// Detaches the checkout at <commit> over its local changes. On this Mac the
/// switch holds out against every signal but SIGKILL (`switch_to`).
fn force_to(site: &mut Site, commit: &str) -> Result<()> {
    match site {
        Site::Local(top) => switch_to(top, commit, true),
        Site::Remote(remote) => remote
            .git(None, &["checkout", "-q", "--detach", "--force", commit])
            .map(|_| ()),
    }
}

/// Deletes the file <path>, in the checkout's git dir.
fn remove(site: &mut Site, path: &str) -> Result<()> {
    match site {
        Site::Local(_) => std::fs::remove_file(path).context(|| format!("removing {path}")),
        Site::Remote(remote) => remote
            .run_in(
                &format!("rm -f {}", sh_quote(path)),
                "removing the scratch index",
            )
            .map(|_| ()),
    }
}

/// The paths of the working tree that differ from <commit>'s tree: changed,
/// deleted, or not in it.
fn differing(site: &mut Site, commit: &str) -> Result<BTreeSet<String>> {
    let git_dir = site.git(None, &["rev-parse", "--absolute-git-dir"])?;
    let index = Path::new(git_dir.trim())
        .join(format!(
            "builds-reset-index-{}",
            crate::text::staged_suffix()
        ))
        .display()
        .to_string();
    let listed = differing_with(site, &index, commit);
    let removed = remove(site, &index);
    let listed = listed?;
    removed?;
    Ok(listed)
}

/// `differing`, with the scratch index file <index>.
fn differing_with(site: &mut Site, index: &str, commit: &str) -> Result<BTreeSet<String>> {
    site.git(Some(index), &["read-tree", commit])?;
    // Differing files make --refresh exit 1 even with -q; the answer is in
    // diff-files below.
    let _ = site.git(Some(index), &["update-index", "-q", "--refresh"]);
    let mut paths = BTreeSet::new();
    for args in [
        &["diff-files", "--name-only", "-z"][..],
        &["ls-files", "--others", "--exclude-standard", "-z"][..],
    ] {
        paths.extend(
            site.git(Some(index), args)?
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_string),
        );
    }
    Ok(paths)
}

/// The dirty paths of the checkout that none of <commits> holds exactly.
fn foreign_changes(site: &mut Site, commits: &[&str]) -> Result<Vec<String>> {
    let mut foreign = paths_in_status(&site.git(None, &UNCOMMITTED_STATUS)?);
    for commit in commits {
        if foreign.is_empty() {
            break;
        }
        let differs = differing(site, commit)?;
        foreign.retain(|path| differs.contains(path));
    }
    Ok(foreign.into_iter().collect())
}

/// <matching>: a commit the dirty paths may match as well, the one a halfway
/// switch was heading to when it is not origin's main today.
pub fn cmd_reset(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    checkout: &str,
    matching: Option<&str>,
) -> Result<()> {
    let Some(lane) = places.app_lane_of(repo, checkout)? else {
        fail!("{checkout} is not one of {repo}'s app checkouts (builds status lists them)");
    };
    let mut site = Site::of(places, ssh, repo, lane, checkout)?;
    let lock = lock::take_checkout(places, repo, lane, checkout)?;
    let mut sent = Vec::new();
    let reset = reset_under_lock(&mut site, places, repo, matching, &mut sent);
    let forgot = forget(&site, &sent);
    unsafe { libc::close(lock) };
    reset?;
    forgot?;
    note(format!("{} is on {TARGET}", site.shown()?));
    Ok(())
}

/// The reset, with the checkout's lock held.
fn reset_under_lock(
    site: &mut Site,
    places: &Places,
    repo: &str,
    matching: Option<&str>,
    sent: &mut Vec<String>,
) -> Result<()> {
    let mut revs = vec![TARGET];
    revs.extend(matching);
    let brought = bring(site, places, repo, &revs, sent)?;
    let target = brought[0].as_str();
    let mut commits = vec![target, "HEAD"];
    commits.extend(brought.get(1).map(String::as_str));
    let foreign = foreign_changes(site, &commits)?;
    if !foreign.is_empty() {
        let also = matching
            .map(|rev| format!(" nor {rev}'s"))
            .unwrap_or_default();
        fail!(
            "{} has changes that are neither {TARGET}'s ({target}) nor HEAD's{also}, so nothing was changed; find whose they are:\n  {}",
            site.shown()?,
            foreign.join("\n  ")
        );
    }
    force_to(site, target)?;
    let left = site.git(None, &["status", "--porcelain"])?;
    if !left.trim().is_empty() {
        fail!("{} is still dirty after the reset:\n{left}", site.shown()?);
    }
    Ok(())
}

#[cfg(test)]
#[path = "app_reset_tests.rs"]
mod tests;
