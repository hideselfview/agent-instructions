//! The running work the sidebar's Work tab lists, computed from the lists the
//! daemon has read and the claim facts joined to each running item: one work
//! per worktree, with what is happening now and what landed. The
//! page draws these and groups nothing itself.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::lists::Stamp;
use crate::lists::view::{Named, View};

/// The running items of one worktree: the work an agent does there.
#[derive(Clone, Debug, Serialize)]
pub struct Work {
    /// The first item's title.
    pub title: String,
    pub repo: String,
    pub checkout: String,
    /// What is happening now, in the agent's words: its claim's note.
    pub now: Option<String>,
    pub agent: Option<String>,
    pub since: Stamp,
    pub branch: String,
    /// The brief's path in the repo.
    pub plan: String,
    /// The running items there.
    pub items: Vec<Named>,
    /// What already landed for them, as 7-character commits.
    pub landed: Vec<String>,
}

fn short(commit: &str) -> String {
    commit.chars().take(7).collect()
}

/// The works of <view>, whose running items carry their claim facts, oldest
/// start first.
pub fn works(view: &View) -> Vec<Work> {
    let mut by_worktree: BTreeMap<&str, Work> = BTreeMap::new();
    for item in &view.items {
        for running in &item.running {
            let work = by_worktree
                .entry(running.worktree.as_str())
                .or_insert_with(|| Work {
                    title: item.title.clone(),
                    repo: running.repo.clone(),
                    checkout: running.checkout.clone(),
                    now: running.claim.as_ref().and_then(|c| c.note.clone()),
                    agent: running.claim.as_ref().and_then(|c| c.agent.clone()),
                    since: running.since.clone(),
                    branch: running.branch.clone(),
                    plan: running.brief.clone(),
                    items: Vec::new(),
                    landed: Vec::new(),
                });
            work.items.push(Named {
                id: item.id,
                title: item.title.clone(),
            });
            for part in &item.landed {
                for commit in &part.commits {
                    let commit = short(commit);
                    if !work.landed.contains(&commit) {
                        work.landed.push(commit);
                    }
                }
            }
        }
    }
    let mut works: Vec<Work> = by_worktree.into_values().collect();
    works.sort_by_key(|work| work.since.at);
    works
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
