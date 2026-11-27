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

use crate::ack::agent::{Caller, Session};
use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::command::Command;
use crate::fail;
use crate::fail::Result;
use crate::lists::running::running_in_any;
use crate::places::Places;

/// Fails when an agent runs `builds <command>` in a claimed side worktree that
/// is off its claim's branch.
///
/// <nested> is whether an agent's builds command started this one: `ack::gate`
/// marks the environment (`PASSED`) of the commands it lets run, so a command
/// that `builds land` runs inside the landing, after it detached HEAD itself,
/// inherits the mark and passes. The caller reads it before `ack::gate` sets it.
pub fn gate(command: Command, nested: bool, session: &Session) -> Result<()> {
    if !applies(command, nested) {
        return Ok(());
    }
    require_claimed_branch(&crate::args::current_dir()?, || session.caller())
}

/// Does the check cover <command>, which <nested> says another builds command
/// started?
fn applies(command: Command, nested: bool) -> bool {
    !nested && command.guards_claimed_branch()
}

/// Fails when <dir> is in a claimed side worktree whose running items'
/// branches do not include the branch checked out. <caller> reads the agent
/// that runs the command, and runs only in a claimed side worktree.
fn require_claimed_branch(
    dir: &Path,
    caller: impl FnOnce() -> Result<Option<Caller>>,
) -> Result<()> {
    let Some(checkout) = Checkout::at_keeping_env(dir)? else {
        return Ok(());
    };
    if !checkout.is_side() || rewriting_history(&checkout) {
        return Ok(());
    }
    let Some(claim) = Claim::read(&checkout.claim_file())? else {
        return Ok(());
    };
    // Only a side worktree's claim needs to know who runs the command, so a
    // commit hook in a main checkout runs with a session and no agent id.
    if caller()?.is_none() {
        return Ok(());
    }
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

impl crate::command::Command {
    /// Whether the claimed-branch check (claimed_branch.rs) covers the
    /// command: an agent runs it before it builds, reviews or lands.
    pub fn guards_claimed_branch(self) -> bool {
        use crate::command::Command::*;
        use crate::command::*;
        match self {
            AppBuild(
                app_build::Build
                | app_build::LandingCoverage
                | app_build::DropShots
                | app_build::RecordHanded,
            )
            | Land
            | Cargo
            | Review(
                review::Run
                | review::Copy
                | review::Settle
                | review::Wait
                | review::Dispute
                | review::Keep
                | review::Show
                | review::Key
                | review::CheckLanding
                | review::Landed
                | review::Groups
                | review::Replay
                | review::Audit
                | review::Misses
                | review::Rates
                | review::Scores
                | review::CasePropose
                | review::CaseAdd,
            )
            | Checks(checks::Run | checks::Whole | checks::Stats | checks::List | checks::Tests)
            | Item(
                item::Add
                | item::Edit
                | item::Note
                | item::Changes
                | item::Wait
                | item::Backlog
                | item::Start
                | item::Take
                | item::CheckBrief
                | item::Handoff
                | item::Stop
                | item::Finished
                | item::Drop
                | item::Place
                | item::CheckLanding
                | item::Landed
                | item::Show
                | item::List
                | item::Times,
            ) => true,
            Help
            | Ack
            | RulesOnEdit
            | Setup
            | Lake
            | Tool
            | New
            | Claim
            | Note
            | Release
            | Retire
            | BuildApp
            | Lock
            | Programs
            | Queue
            | Takeover
            | AppClone
            | SccacheTmpdir
            | Prune
            | Du
            | Status
            | Health
            | InstallHook
            | Waits
            | Brief
            | Install
            | Version
            | Next
            | Agent(agent::Start | agent::Run | agent::Resume | agent::List | agent::Show)
            | Shots(shots::Findings | shots::Picture | shots::CheckProgram | shots::Checker)
            | AppCheckout(app_checkout::Add | app_checkout::Remove | app_checkout::Reset)
            | Host(
                host::Add
                | host::Setup
                | host::SetupHere
                | host::SpeedHere
                | host::Band
                | host::List
                | host::Cache
                | host::Remove
                | host::ClearXcode,
            )
            | Remote(remote::Run | remote::Step)
            | Units(units::Prune | units::GitCheckouts | units::AfterBuild)
            | Dashboard(dashboard::Stage | dashboard::Serve)
            | Judge(judge::Listen)
            | Question(
                question::Ask
                | question::Answer
                | question::Settle
                | question::Done
                | question::Drop
                | question::Place
                | question::Edit
                | question::Show
                | question::List,
            )
            | Area(area::Add | area::Rename | area::Move | area::Remove)
            | Decision(
                decision::Add | decision::Edit | decision::Find | decision::Show | decision::List,
            )
            | Design(
                design::Places
                | design::Only
                | design::Record
                | design::Amend
                | design::Approve
                | design::Check
                | design::Landed
                | design::Title
                | design::Drop
                | design::Show
                | design::List
                | design::Render
                | design::TokensCss
                | design::PublishFind
                | design::PublishFiles
                | design::PublishLast
                | design::Listen
                | design::PublishTake
                | design::PublishDone
                | design::PublishFailed,
            )
            | Lists(
                lists::Init | lists::DesignGlobs | lists::AddRepo | lists::Show | lists::ProjectOf,
            ) => false,
        }
    }
}

#[cfg(test)]
#[path = "claimed_branch_tests.rs"]
mod tests;
