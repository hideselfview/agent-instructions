//! Where an item runs. The agent is the claim of the item's worktree, never
//! stored on the item, so the two can't disagree; this reads it. The release
//! guard also lives here: a worktree can't be released while an item runs in
//! it. `findings` is what `builds health` says about the lists.

use std::path::Path;

use crate::checkout::{Checkout, Root};
use crate::claim::Claim;
use crate::fail;
use crate::fail::Result;
use crate::lists::store::{Loaded, Store, project_of};
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
    agent_in(Checkout::at(path)?.as_ref())
}

/// The agent of an item running in <checkout> (None: no live worktree there).
fn agent_in(checkout: Option<&Checkout>) -> Result<Agent> {
    let Some(checkout) = checkout else { return Ok(Agent::WorktreeGone) };
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
    lists.items.iter().filter(|item| item.running.iter().any(|r| r.worktree == worktree)).collect()
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

/// What `builds health` says about the lists: a store that fails its check,
/// and a running item whose worktree is gone or has no claim. `builds release`
/// refuses while an item runs in the worktree, so only a hand edit reaches the
/// second; it is named loudly, never repaired. One line per fault.
pub fn findings(loaded: &std::result::Result<Vec<Loaded>, String>, roots: &[Root]) -> Result<Vec<String>> {
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(fault) => return Ok(vec![format!("lists: {fault}")]),
    };
    let mut findings = Vec::new();
    for project in loaded {
        let lists = match &project.lists {
            Ok(lists) => lists,
            Err(fault) => {
                findings.push(format!("lists: project {} fails its check: {fault}", project.project));
                continue;
            }
        };
        for item in &lists.items {
            for running in &item.running {
                let live = roots
                    .iter()
                    .filter_map(|root| root.live.as_ref())
                    .find(|checkout| checkout.top == Path::new(&running.worktree));
                let fault = match agent_in(live)? {
                    Agent::WorktreeGone => "is not a live worktree",
                    Agent::Unclaimed => "has no claim",
                    Agent::Claimed(_) => continue,
                };
                findings.push(format!(
                    "lists: item {} \"{}\" runs in {}, which {fault}; `builds item stop {}` ends it",
                    item.id.0, item.title, running.worktree, item.id.0
                ));
            }
        }
    }
    Ok(findings)
}

#[cfg(test)]
#[path = "running_tests.rs"]
mod tests;
