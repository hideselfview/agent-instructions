//! What a change is to the review's record and to `builds land`: its commits,
//! each with its patch id, which `git patch-id --stable` takes of the commit's
//! diff without context lines, so the id covers only the lines the commit adds
//! and removes. A rebase or a cherry-pick that keeps those lines keeps the
//! patch id while the SHA changes, even when the lines around them changed, so
//! a landing is matched to the review that read the same changes, not the same
//! SHAs. A conflict's resolution that changes an added or removed line gives
//! the commit another patch id.

use std::path::Path;
use std::process::Stdio;

use crate::checkout::git;
use crate::fail;
use crate::fail::{Context, Result};

/// A commit and the patch id of its added and removed lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub patch_id: String,
}

/// Whether two changes are the same: the same patch ids in the same order.
pub fn same_change(a: &[Commit], b: &[Commit]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.patch_id == y.patch_id)
}

/// What <args> prints in <repo>, or why it failed.
fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    let output = git(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        fail!(
            "git {} failed in {}: {}",
            args.join(" "),
            repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The full SHA of each of <revs>, in order; a rev that names no commit fails.
fn full_shas(repo: &Path, revs: &[String]) -> Result<Vec<String>> {
    revs.iter()
        .map(|rev| {
            Ok(git_text(
                repo,
                &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
            )?
            .trim()
            .to_string())
        })
        .collect()
}

/// The commits past <base> up to <head>, oldest first.
pub fn in_range(repo: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    Ok(
        git_text(repo, &["rev-list", "--reverse", &format!("{base}..{head}")])?
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

/// Whether the repo at <repo> has the commit <sha>.
pub fn exists(repo: &Path, sha: &str) -> bool {
    git(repo)
        .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The newest <count> commits of the repo's HEAD, oldest first.
pub fn newest(repo: &Path, count: usize) -> Result<Vec<String>> {
    Ok(git_text(
        repo,
        &[
            "rev-list",
            "--reverse",
            &format!("--max-count={count}"),
            "HEAD",
        ],
    )?
    .lines()
    .map(str::to_string)
    .collect())
}

/// Each of <revs> with the patch id of its added and removed lines, in the
/// order given. A commit that changes nothing (and a merge, whose change `git
/// log -p` does not show) has no patch id and is left out.
pub fn with_patch_ids(repo: &Path, revs: &[String]) -> Result<Vec<Commit>> {
    if revs.is_empty() {
        return Ok(Vec::new());
    }
    let shas = full_shas(repo, revs)?;
    // The diff options are fixed here, not left to the repo's config, so a
    // commit's patch id is the same whenever it is read. `--unified=0` leaves
    // out the context lines, and `git patch-id` ignores the line numbers in
    // the hunk headers, so lines that moved or changed around the change
    // leave its patch id as it was.
    let mut log = git(repo);
    log.args([
        "log",
        "--no-walk=unsorted",
        "-p",
        "--unified=0",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        "--diff-algorithm=myers",
        "--format=commit %H",
    ])
    .args(&shas)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    let mut log = log.spawn().context(|| "running git log".to_string())?;
    let diffs = log.stdout.take().expect("stdout is piped");
    let output = git(repo)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::from(diffs))
        .stderr(Stdio::piped())
        .output()
        .context(|| "running git patch-id".to_string())?;
    let logged = log.wait().context(|| "running git log".to_string())?;
    if !logged.success() {
        fail!("git log of {} failed in {}", shas.join(" "), repo.display());
    }
    if !output.status.success() {
        fail!(
            "git patch-id failed in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let ids: Vec<(&str, &str)> = text
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(patch, sha)| (sha, patch))
        .collect();
    Ok(shas
        .iter()
        .filter_map(|sha| {
            ids.iter().find(|(s, _)| s == sha).map(|(_, patch)| Commit {
                sha: sha.clone(),
                patch_id: patch.to_string(),
            })
        })
        .collect())
}

/// Every path a commit of <revs> adds, changes or deletes (both paths of a
/// rename), each once, in the order first touched.
pub fn paths(repo: &Path, revs: &[String]) -> Result<Vec<String>> {
    if revs.is_empty() {
        return Ok(Vec::new());
    }
    let shas = full_shas(repo, revs)?;
    let mut args: Vec<&str> = vec![
        "log",
        "--no-walk=unsorted",
        "--no-renames",
        "--name-only",
        "--format=",
    ];
    args.extend(shas.iter().map(String::as_str));
    let mut all: Vec<String> = Vec::new();
    for line in git_text(repo, &args)?.lines() {
        if !line.is_empty() && !all.iter().any(|p| p == line) {
            all.push(line.to_string());
        }
    }
    Ok(all)
}

/// The paths each of <commits> adds, changes or deletes, in the order given.
pub fn paths_of_each(repo: &Path, commits: &[Commit]) -> Result<Vec<Vec<String>>> {
    commits
        .iter()
        .map(|commit| paths(repo, std::slice::from_ref(&commit.sha)))
        .collect()
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
