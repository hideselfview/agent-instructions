//! The check that an agent's builds command runs in a claimed side worktree
//! on its claim's branch. A claim names the branch of the item running in the
//! worktree (`builds item take --branch B`). An agent that resumes in a
//! worktree after it was released and handed to another agent finds it on
//! another branch, or on a detached HEAD, and would commit on the new
//! holder's work. Agents run `builds` before each build, review and landing,
//! so the next such command refuses. A rebase or a cherry-pick in progress
//! passes, since it detaches HEAD on purpose. So do the person's own commands
//! (no agent), the main checkouts, worktrees with no claim, and claims whose
//! item has not started (no branch yet).

use std::path::Path;

use crate::ack::agent::Caller;
use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::fail;
use crate::fail::Result;
use crate::lists::running::running_in_any;
use crate::places::Places;

/// The commands the check guards: the ones an agent runs before it builds,
/// reviews or lands.
const GUARDED: [&str; 6] = ["app-build", "review", "land", "cargo", "checks", "item"];

/// Fails when an agent runs `builds <command>` in a claimed side worktree that
/// is off its claim's branch.
pub fn gate(command: &str) -> Result<()> {
    if !GUARDED.contains(&command) || Caller::from_env()?.is_none() {
        return Ok(());
    }
    require_claimed_branch(&crate::lists::args::current_dir()?)
}

/// Fails when <dir> is in a claimed side worktree whose running items'
/// branches do not include the branch checked out.
fn require_claimed_branch(dir: &Path) -> Result<()> {
    let Some(checkout) = Checkout::at_keeping_env(dir)? else {
        return Ok(());
    };
    if !checkout.is_side() || rewriting_history(&checkout) {
        return Ok(());
    }
    let Some(claim) = Claim::read(&checkout.claim_file())? else {
        return Ok(());
    };
    let places = Places::from_env()?;
    let every = crate::lists::store::every_lists(&places)?;
    let path = checkout.top.display().to_string();
    let claimed: Vec<String> = running_in_any(&every, &path)
        .into_iter()
        .flat_map(|(_, item)| item.running.iter())
        .filter(|running| running.worktree == path)
        .map(|running| running.branch.clone())
        .collect();
    match refusal(&path, &claim, &claimed, checkout.branch()?.as_deref()) {
        Some(text) => fail!("{text}"),
        None => Ok(()),
    }
}

/// Does git hold a rebase, cherry-pick or revert open in <checkout>? Each
/// moves HEAD off the claimed branch while it runs.
fn rewriting_history(checkout: &Checkout) -> bool {
    [
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ]
    .iter()
    .any(|name| checkout.git_dir.join(name).exists())
}

/// The refusal for a worktree at <path> claimed by <claim>, whose running
/// items' branches are <claimed>, when it has <current> checked out; None
/// when the commit may go ahead.
fn refusal(path: &str, claim: &Claim, claimed: &[String], current: Option<&str>) -> Option<String> {
    let first = claimed.first()?;
    if current.is_some_and(|branch| claimed.iter().any(|name| name == branch)) {
        return None;
    }
    let on = match current {
        Some(branch) => format!("is on branch {branch}"),
        None => "has a detached HEAD".to_string(),
    };
    Some(format!(
        "refusing to run: {path} is claimed by {} for branch {first}, but it {on}. Another agent holds this worktree. Do not work here: ask the coordinator for a worktree of your own (`builds new --for <who>`), and move your work there with `git switch` on a branch you made or `git cherry-pick`",
        claim.who,
    ))
}

#[cfg(test)]
#[path = "claimed_branch_tests.rs"]
mod tests;
