//! What a worktree held when a step ran there: HEAD's commit, the branch
//! checked out, and the uncommitted changes, which `builds item times` uses
//! to tell a fix from a flaky pass and to mark a step that ran on a commit
//! off its run's branch.

use std::io::Read;
use std::path::Path;

use crate::side_git::{UNCOMMITTED_STATUS, ask, paths_in_status, run};

/// The most bytes of one changed file that `read` hashes.
const HASHED_BYTES: u64 = 4 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeState {
    /// HEAD's commit, in full.
    pub head: String,
    /// The branch checked out; None for a detached HEAD.
    pub branch: Option<String>,
    /// A hash of the uncommitted changes, staged, changed and untracked
    /// alike: each changed path with its content; None for a clean tree.
    pub changes: Option<String>,
}

/// The 64-bit FNV-1a hash of <bytes>, continued from <hash>.
fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// What the worktree at <top> holds now, or what git said when it failed.
pub fn read(top: &Path) -> Result<TreeState, String> {
    let head = run(top, &["rev-parse", "HEAD"])?.trim().to_string();
    // `symbolic-ref -q` says no for a detached HEAD.
    let branch =
        ask(top, &["symbolic-ref", "--short", "-q", "HEAD"])?.map(|out| out.trim().to_string());
    let paths = paths_in_status(&run(top, &UNCOMMITTED_STATUS)?);
    let changes = if paths.is_empty() {
        None
    } else {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for path in &paths {
            hash = fnv(hash, path.as_bytes());
            hash = fnv(hash, b"\0");
            let file = top.join(path);
            match std::fs::File::open(&file) {
                Ok(opened) => {
                    let mut content = Vec::new();
                    opened
                        .take(HASHED_BYTES)
                        .read_to_end(&mut content)
                        .map_err(|error| format!("reading {}: {error}", file.display()))?;
                    hash = fnv(hash, &content);
                }
                // A deleted path hashes as its name alone.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    hash = fnv(hash, b"\x01deleted")
                }
                Err(error) => return Err(format!("opening {}: {error}", file.display())),
            }
        }
        Some(format!("{hash:016x}"))
    };
    Ok(TreeState {
        head,
        branch,
        changes,
    })
}

/// Is <head> a commit of the run's branch <run_branch> or of origin/main
/// since <run_branch> left it, in the repo at <top>? A commit before the
/// branch started, or on another branch, is off it (a bisect or an old
/// checkout). <branch> is the branch checked out at <head>: the run's own
/// branch needs no question to git.
pub fn on_run_branch(
    top: &Path,
    head: &str,
    branch: Option<&str>,
    run_branch: &str,
) -> Result<bool, String> {
    if branch == Some(run_branch) {
        return Ok(true);
    }
    let fork = run(top, &["merge-base", run_branch, "origin/main"])?;
    let fork = fork.trim();
    let ancestor = |older: &str, newer: &str| {
        ask(top, &["merge-base", "--is-ancestor", older, newer]).map(|yes| yes.is_some())
    };
    Ok(ancestor(fork, head)? && (ancestor(head, run_branch)? || ancestor(head, "origin/main")?))
}

#[cfg(test)]
#[path = "tree_state_tests.rs"]
mod tests;
