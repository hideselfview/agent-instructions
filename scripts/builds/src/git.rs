//! What git says about commits and revisions of a repo, and the steps that
//! throw away a worktree's changes and a branch. Every caller reads the same
//! answers and errors from here: a revision that names nothing is `None`, and
//! any other git failure is an error that quotes git.

use std::path::Path;
use std::process::Stdio;

use crate::fail;
use crate::fail::{Context, Result};

/// What <args> prints in <repo>, or why it failed.
pub fn text(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(boards::git_text(
        boards::git(repo).args(args),
        &format!("run git {} in {}", args.join(" "), repo.display()),
    )?)
}

/// The text of <path> in the commit or tree <rev> of <repo>, or None when
/// <rev> has no file there (a submodule is no file).
pub fn file_text(repo: &Path, rev: &str, path: &str) -> Result<Option<String>> {
    let entry = text(repo, &["ls-tree", "-z", "--full-tree", rev, "--", path])?;
    let blob = entry.split('\0').find_map(|line| {
        let (meta, named) = line.split_once('\t')?;
        let mut fields = meta.split(' ');
        let (_, kind, sha) = (fields.next()?, fields.next()?, fields.next()?);
        (named == path && kind == "blob").then_some(sha)
    });
    match blob {
        Some(sha) => text(repo, &["cat-file", "blob", sha]).map(Some),
        None => Ok(None),
    }
}

/// The full object ID of <rev>, or None when Git cannot find it. Quiet
/// verification exits 1 both for a missing revision and for a broken ref,
/// but emits a diagnostic for the broken ref.
pub fn resolve_revision(dir: &Path, rev: &str) -> Result<Option<String>> {
    let output = boards::git(dir)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options", rev])
        .stdin(Stdio::null())
        .output()
        .context(|| format!("Git could not start in {} to resolve {rev}", dir.display()))?;
    match output.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        )),
        Some(1) if output.stderr.is_empty() => Ok(None),
        _ => {
            let error = format!(
                "Git could not resolve {rev} in {} ({}).\n{}",
                dir.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            fail!("{}", error.trim_end());
        }
    }
}

/// The immutable tip of a named local branch, or None when it is absent.
pub fn branch_tip(dir: &Path, branch: &str) -> Result<Option<String>> {
    resolve_revision(dir, &format!("refs/heads/{branch}"))
}

/// Does branch <branch> exist in the repo of <dir>?
pub fn branch_exists(dir: &Path, branch: &str) -> Result<bool> {
    Ok(branch_tip(dir, branch)?.is_some())
}

/// Detaches the worktree <top> at origin/main, discarding the files not
/// committed (ignored files, such as the build links, stay).
pub fn detach_at_main(top: &Path) -> Result<()> {
    boards::git_text(
        boards::git(top).args([
            "-c",
            "core.hooksPath=/dev/null",
            "checkout",
            "-q",
            "-f",
            "--detach",
            "origin/main",
        ]),
        "detach the worktree at origin/main",
    )?;
    boards::git_text(
        boards::git(top).args(["clean", "-f", "-d", "-q"]),
        "remove the files not committed",
    )?;
    Ok(())
}

/// Deletes <branch> of the repo at <repo>, when the repo still has it.
pub fn delete_branch(repo: &Path, branch: &str) -> Result<()> {
    if branch_exists(repo, branch)? {
        text(repo, &["branch", "-q", "-D", branch])?;
    }
    Ok(())
}

/// Whether the repo at <repo> has the commit <sha>.
pub fn commit_exists(repo: &Path, sha: &str) -> Result<bool> {
    // `cat-file -e` exits 128 when it cannot resolve an absent commit with
    // the `^{commit}` suffix. Quiet rev-parse returns 1 without output then.
    Ok(resolve_revision(repo, &format!("{sha}^{{commit}}"))?.is_some())
}

/// The commits past <base> up to <head>, oldest first.
pub fn commits_in_range(repo: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    Ok(
        text(repo, &["rev-list", "--reverse", &format!("{base}..{head}")])?
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
