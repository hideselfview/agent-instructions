//! What the page's three tabs hold, computed from the lists the daemon has
//! read and the claim facts joined to each running item: which running work
//! waits on the person and which runs, and each tab's count. The page draws
//! these and counts nothing itself.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::lists::Stamp;
use crate::lists::view::{ItemView, Named, RunningView, View};

/// The tab a running work is under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tab {
    /// The person has to act before the work goes on.
    Waiting,
    Running,
}

/// The state a running work is grouped under on the Running tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    Working,
    /// Commits wait to land.
    ReadyToLand,
}

/// The running items of one worktree: the work an agent does there.
#[derive(Clone, Debug, Serialize)]
pub struct Work {
    /// The first item's title.
    pub title: String,
    pub repo: String,
    pub checkout: String,
    pub tab: Tab,
    pub group: Group,
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

/// The group of a work whose worktree has <claim>: commits waiting to land
/// put it under Ready to land unless its note holds it back.
fn group_of(running: &RunningView) -> Group {
    let Some(claim) = &running.claim else {
        return Group::Working;
    };
    let held = matches!(claim.note_state.as_deref(), Some("held" | "waiting-on-you"));
    if claim.waiting_to_land > 0 && !held {
        Group::ReadyToLand
    } else {
        Group::Working
    }
}

fn short(commit: &str) -> String {
    commit.chars().take(7).collect()
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Board {
    /// Questions on the stack, design changes to review, and running work
    /// that waits on the person.
    pub waiting: usize,
    /// Running work that does not wait on the person, and design changes the
    /// person approved that the design session has not landed yet.
    pub running: usize,
    /// Open items no agent has started.
    pub queue: usize,
    pub works: Vec<Work>,
}

/// Does <item> wait on an answer or a review? A wait on another item or a
/// condition is not the person's.
fn waits_on_person(item: &ItemView) -> bool {
    item.waits
        .iter()
        .any(|wait| !wait.met && matches!(wait.kind, "question" | "design"))
}

/// The board of <view>, whose running items carry their claim facts.
pub fn build(view: &View) -> Board {
    let mut by_worktree: BTreeMap<&str, Work> = BTreeMap::new();
    let mut person_waits: Vec<&str> = Vec::new();
    for item in &view.items {
        for running in &item.running {
            let work = by_worktree
                .entry(running.worktree.as_str())
                .or_insert_with(|| Work {
                    title: item.title.clone(),
                    repo: running.repo.clone(),
                    checkout: running.checkout.clone(),
                    tab: Tab::Running,
                    group: group_of(running),
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
            let noted = running
                .claim
                .as_ref()
                .and_then(|claim| claim.note_state.as_deref())
                == Some("waiting-on-you");
            if (noted || waits_on_person(item))
                && !person_waits.contains(&running.worktree.as_str())
            {
                person_waits.push(running.worktree.as_str());
            }
        }
    }
    for worktree in person_waits {
        by_worktree.get_mut(worktree).expect("added above").tab = Tab::Waiting;
    }
    let works: Vec<Work> = by_worktree.into_values().collect();
    let count = |tab| works.iter().filter(|work| work.tab == tab).count();
    let open = |approved: bool| {
        view.designs
            .iter()
            .filter(|design| design.closed.is_none() && design.approved.is_some() == approved)
            .count()
    };
    Board {
        waiting: view.stack.len() + open(false) + count(Tab::Waiting),
        running: count(Tab::Running) + open(true),
        queue: view
            .items
            .iter()
            .filter(|item| item.running.is_empty())
            .count(),
        works,
    }
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
