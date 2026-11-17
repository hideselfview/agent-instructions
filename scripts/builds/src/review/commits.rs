//! The git reads a review makes of a branch's commits: their range, the paths
//! they change, and whether the repo has a commit.

use std::path::Path;
use std::process::Stdio;

use crate::checkout::git;
use crate::fail;
use crate::fail::{Context, Result};

/// What <args> prints in <repo>, or why it failed.
pub fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
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

/// The paths each of <shas> adds, changes or deletes, in the order given.
pub fn paths_of_each(repo: &Path, shas: &[String]) -> Result<Vec<Vec<String>>> {
    shas.iter()
        .map(|sha| paths(repo, std::slice::from_ref(sha)))
        .collect()
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
