//! What the page's three tabs hold, computed from the lists the daemon has
//! read and the claim facts joined to each running item: which running work
//! waits on the person and which runs, and each tab's count. The page draws
//! these and counts nothing itself.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::lists::Id;
use crate::lists::view::{ItemView, View};

/// The tab a running work is under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tab {
    /// The person has to act before the work goes on.
    Waiting,
    Running,
}

/// The running items of one worktree: the work an agent does there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Work {
    pub repo: String,
    pub checkout: String,
    pub tab: Tab,
    /// The running items there, by id.
    pub items: Vec<Id>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
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
                    repo: running.repo.clone(),
                    checkout: running.checkout.clone(),
                    tab: Tab::Running,
                    items: Vec::new(),
                });
            work.items.push(item.id);
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
