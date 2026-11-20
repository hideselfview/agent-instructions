//! The running work and tab counts the sidebar lists, computed from the lists the
//! daemon has read and the claim facts joined to each running item: one work
//! per worktree, with what is happening now and what landed. The
//! page draws these and groups nothing itself.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::lists::Stamp;
use crate::lists::derive::Times;
use crate::lists::view::{DesignView, ItemView, Named, RulesReview, View};

/// The tab a running work is under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tab {
    /// The person has to act before the work goes on.
    Waiting,
    Running,
}

/// What Waiting on you counts for the project's tab: the stack's questions, the
/// design changes waiting on the person's review and the running work held on
/// the person.
/// The page counts the other tabs from the rows it draws.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Board {
    pub waiting: usize,
    pub works: Vec<Work>,
}

/// The running items of one worktree: the work an agent does there.
#[derive(Clone, Debug, Serialize)]
pub struct Work {
    /// What the work is called: the title of its one item, or for a work of
    /// several items the job its agent claimed the worktree for (`job`). A work
    /// of several items whose worktree has no agent name takes its first
    /// item's title.
    pub title: String,
    pub repo: String,
    pub checkout: String,
    /// What is happening now, in the agent's words: its claim's note.
    pub now: Option<String>,
    pub agent: Option<String>,
    pub since: Stamp,
    /// What the runs on its branch add up to.
    pub times: Times,
    pub tab: Tab,
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

/// The job that `agent` names: the name a coordinator gave the worktree when
/// it claimed it, without the " agent" that ends most names, so "screen
/// reads library agent" names the job "screen reads library".
fn job(agent: &str) -> &str {
    agent.strip_suffix(" agent").unwrap_or(agent)
}

/// Does <design> wait on the person's review? It waits on their approval
/// until they give it, and on their review only once its rules review takes
/// its branch (decision #362).
pub fn to_review(design: &DesignView) -> bool {
    design.closed.is_none()
        && design.approved.is_none()
        && design.rules_review == Some(RulesReview::Read)
}

/// Does <item> wait on the person's answer or review
/// (`derive::wait_on_person`)?
fn waits_on_person(item: &ItemView) -> bool {
    item.waits.iter().any(|wait| wait.on_person())
}

/// The tab counts of <view>.
pub fn board(view: &View) -> Board {
    let works = works(view);
    let count = |tab| works.iter().filter(|work| work.tab == tab).count();
    let to_review = view.designs.iter().filter(|d| to_review(d)).count();
    Board {
        waiting: view.stack.len() + to_review + count(Tab::Waiting),
        works,
    }
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
                    times: running.times.clone(),
                    tab: Tab::Running,
                    branch: running.branch.clone(),
                    plan: running.brief.clone(),
                    items: Vec::new(),
                    landed: Vec::new(),
                });
            if waits_on_person(item)
                || running.claim.as_ref().and_then(|c| c.note_state.as_deref())
                    == Some("waiting-on-you")
            {
                work.tab = Tab::Waiting;
            }
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
    for work in &mut works {
        if work.items.len() > 1
            && let Some(agent) = &work.agent
        {
            work.title = job(agent).to_string();
        }
    }
    works.sort_by_key(|work| work.since.at);
    works
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
