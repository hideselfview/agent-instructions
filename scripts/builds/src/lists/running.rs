//! Where an item runs. The agent is the claim of the item's worktree, never
//! stored on the item, so the two can't disagree; this reads it. The release
//! guard also lives here: a worktree can't be released while an item runs in
//! it.

use std::path::Path;

use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::fail;
use crate::fail::Result;
use crate::lists::store::{Store, project_of};
use crate::lists::{Item, Lists, Running, side_worktree_parts};
use crate::places::Places;

/// Who holds the worktree an item runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Agent {
    Claimed(String),
    Unclaimed,
    WorktreeGone,
}

impl Agent {
    pub fn words(&self) -> String {
        match self {
            Agent::Claimed(who) => who.clone(),
            Agent::Unclaimed => "no claim".to_string(),
            Agent::WorktreeGone => "worktree gone".to_string(),
        }
    }
}

pub fn agent_of(running: &Running) -> Result<Agent> {
    let path = Path::new(&running.worktree);
    if !path.is_dir() {
        return Ok(Agent::WorktreeGone);
    }
    let Some(checkout) = Checkout::at(path)? else { return Ok(Agent::WorktreeGone) };
    Ok(match Claim::read(&checkout.claim_file())? {
        Some(claim) if !claim.who.is_empty() => Agent::Claimed(claim.who),
        _ => Agent::Unclaimed,
    })
}

/// The line that says what runs where, the one to read after a compact:
/// `Running: <agent>, <repo> <side-N> <branch>, brief <path>`.
pub fn marker(running: &Running) -> Result<String> {
    let (repo, checkout) = side_worktree_parts(&running.worktree)
        .unwrap_or_else(|| ("?".to_string(), running.worktree.clone()));
    Ok(format!(
        "Running: {}, {repo} {checkout} {}, brief {}",
        agent_of(running)?.words(),
        running.branch,
        running.brief
    ))
}

/// The open items running in <worktree>.
pub fn running_in<'a>(lists: &'a Lists, worktree: &str) -> Vec<&'a Item> {
    lists.items.iter().filter(|item| item.running.as_ref().is_some_and(|r| r.worktree == worktree)).collect()
}

/// `builds release` is refused while an item runs in the worktree: the claim
/// is what names the agent, so releasing it under a running item would leave
/// the marker with no one behind it.
pub fn require_release_allowed(places: &Places, checkout: &Checkout) -> Result<()> {
    let Some(project) = project_of(places, checkout)? else { return Ok(()) };
    let lists = Store::named(places, &project)?.load()?;
    let top = checkout.top.display().to_string();
    let running = running_in(&lists, &top);
    if running.is_empty() {
        return Ok(());
    }
    let named: Vec<String> = running.iter().map(|item| format!("{} \"{}\"", item.id, item.title)).collect();
    let first = running[0].id.0;
    fail!(
        "{top} still runs {}: land it (land.sh --done {first}), stop it (builds item stop {first} --why WHY) or hand it \
         to the next agent (builds item handoff {first} --brief PATH) before releasing the worktree",
        named.join("; ")
    );
}
