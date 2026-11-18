//! The lists JSON the page reads and `lists show --json` prints: the whole
//! state with the derived fields joined in, less the claim fields the daemon
//! adds (agent, note, note state, commits waiting to land).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::fail::Result;
use crate::lists::derive::{self, FLOW_DAYS, FlowDay, Times};
use crate::lists::trail::{Trail, trail};
use crate::lists::{
    Audience, DesignChange, Effect, Id, Item, LandedPart, Lists, Located, Note, Placement, PlanRef,
    Priority, QuestionKind, Resolution, Stamp, Standing, Wait, side_worktree_parts,
};

#[derive(Clone, Debug, Serialize)]
pub struct View {
    pub project: String,
    pub version: u64,
    /// What is wrong with the store, when it fails its check; then nothing
    /// else is filled.
    pub error: Option<String>,
    pub stack: Vec<StackView>,
    pub areas: Vec<String>,
    pub items: Vec<ItemView>,
    pub designs: Vec<DesignView>,
    pub decisions: Vec<DecisionView>,
    pub flow: Vec<FlowDay>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Named {
    pub id: Id,
    pub title: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StackView {
    pub id: Id,
    pub title: String,
    pub kind: QuestionKind,
    pub effect: Effect,
    pub details: String,
    /// The open items that wait on it directly.
    pub blocks: Vec<BlockedView>,
    pub blocked_count: usize,
    pub blocks_nothing: Option<String>,
    pub placed: Option<Placement>,
    pub plan: Option<PlanRef>,
    pub opened: Stamp,
    /// Where it came from (trail.rs).
    pub trail: Trail,
}

/// An open item that waits on a question directly.
#[derive(Clone, Debug, Serialize)]
pub struct BlockedView {
    pub id: Id,
    pub title: String,
    pub priority: Option<Priority>,
    /// How many open items wait on it directly or through a chain of open
    /// items.
    pub behind: usize,
    pub opened: Stamp,
}

#[derive(Clone, Debug, Serialize)]
pub struct WaitView {
    /// `item`, `question`, `design` or `condition`.
    pub kind: &'static str,
    pub id: Option<Id>,
    pub title: Option<String>,
    pub text: Option<String>,
    pub met: bool,
    /// Does it wait on the person (`derive::wait_on_person`)?
    pub on_person: bool,
}

/// What the daemon joins to a running item from the worktree's claim and
/// git; `lists show --json` has none of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClaimFacts {
    /// The claim's who; None when the worktree has no claim.
    pub agent: Option<String>,
    pub note: Option<String>,
    pub note_state: Option<String>,
    /// Commits on the worktree's HEAD that origin/main has no copy of.
    pub waiting_to_land: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunningView {
    pub worktree: String,
    pub repo: String,
    pub checkout: String,
    pub branch: String,
    pub brief: String,
    pub since: Stamp,
    /// What the runs on its branch add up to, the runs of other items on
    /// the branch included.
    pub times: Times,
    #[serde(flatten)]
    pub claim: Option<ClaimFacts>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ItemView {
    pub id: Id,
    pub title: String,
    pub area: String,
    pub details: String,
    #[serde(rename = "for")]
    pub r#for: Audience,
    pub state: derive::ItemState,
    pub priority: Option<Priority>,
    /// How many open items wait on it directly or through a chain of open
    /// items.
    pub behind: usize,
    pub brief: Option<String>,
    pub waits: Vec<WaitView>,
    /// One per repo the item runs in; empty when it is not running.
    pub running: Vec<RunningView>,
    /// One per repo the item is parked in: its work waits there with no
    /// agent. Empty when it is not parked.
    pub parked: Vec<crate::lists::parked::Parked>,
    pub landed: Vec<LandedPart>,
    /// What its runs add up to; None when it never ran.
    pub times: Option<Times>,
    pub notes: Vec<Note>,
    pub opened: Stamp,
    /// Where it came from (trail.rs).
    pub trail: Trail,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlaceItem {
    pub id: Id,
    pub title: String,
    /// The item's state, or `closed`.
    pub state: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlaceView {
    pub path: String,
    pub item: Option<PlaceItem>,
    pub matches: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DesignView {
    pub id: Id,
    pub title: String,
    pub details: String,
    pub repo: String,
    pub places: Vec<PlaceView>,
    /// The board files it deletes.
    pub deleted: Vec<String>,
    /// The open items that wait on it.
    pub blocks: Vec<Named>,
    pub no_app_place: Option<String>,
    /// The person's review: None while it waits on them.
    pub approved: Option<Stamp>,
    /// The person's decision that settled a board edit of an existing
    /// screen, which approved the change when the implementing agent
    /// recorded it.
    pub decision: Option<Named>,
    /// Whether the session that recorded it was told of the approval.
    pub session: crate::lists::design_session::SessionState,
    pub opened: Stamp,
    pub landed: Vec<crate::lists::DesignLanding>,
    /// When it landed, for one that did in the last `FLOW_DAYS` days.
    pub closed: Option<Stamp>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkView {
    pub id: Id,
    pub title: String,
    pub open: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DecisionView {
    pub id: Id,
    pub date: String,
    pub title: String,
    pub by: String,
    pub body: String,
    pub question: Option<Id>,
    pub work: Vec<WorkView>,
    pub replaces: Option<Id>,
    pub replaced_by: Option<Id>,
    pub plan: Option<PlanRef>,
}

impl View {
    /// What the page shows for a store that fails its check.
    pub fn failed(project: &str, version: u64, error: String) -> View {
        View {
            project: project.to_string(),
            version,
            error: Some(error),
            stack: Vec::new(),
            areas: Vec::new(),
            items: Vec::new(),
            designs: Vec::new(),
            decisions: Vec::new(),
            flow: Vec::new(),
        }
    }

    /// The view of <lists> at <now>. Fails when `trail::trail` fails on a
    /// record's trail.
    pub fn build(lists: &Lists, now: i64) -> Result<View> {
        Ok(View {
            project: lists.project.clone(),
            version: lists.version,
            error: None,
            stack: derive::stack_order(lists)
                .into_iter()
                .map(|entry| {
                    Ok(StackView {
                        id: entry.question.id,
                        title: entry.question.title.clone(),
                        kind: entry.question.kind,
                        effect: entry.question.effect,
                        details: entry.question.details.clone(),
                        blocks: entry
                            .blocks
                            .iter()
                            .map(|blocked| BlockedView {
                                id: blocked.item.id,
                                title: blocked.item.title.clone(),
                                priority: blocked.item.priority.clone(),
                                behind: blocked.behind,
                                opened: blocked.item.opened.clone(),
                            })
                            .collect(),
                        blocked_count: entry.blocked_count,
                        blocks_nothing: entry.question.blocks_nothing.clone(),
                        placed: entry.question.placed.clone(),
                        plan: entry.question.plan.clone(),
                        opened: entry.question.opened.clone(),
                        trail: trail(lists, entry.question.id)?,
                    })
                })
                .collect::<Result<_>>()?,
            areas: lists.areas.clone(),
            items: {
                let branches = derive::branch_times(lists)?;
                lists
                    .items
                    .iter()
                    .map(|item| item_view(lists, item, &branches))
                    .collect::<Result<_>>()?
            },
            designs: designs(lists, now),
            decisions: decisions(lists),
            flow: derive::flow(lists, now),
        })
    }
}

fn wait_view(lists: &Lists, wait: &Wait) -> WaitView {
    let named = |kind: &'static str, id: Id| {
        let title = lists.locate(id).map(|found| found.title.to_string());
        WaitView {
            kind,
            id: Some(id),
            title,
            text: None,
            met: derive::wait_met(lists, wait),
            on_person: derive::wait_on_person(lists, wait),
        }
    };
    match wait {
        Wait::Item(id) => named("item", *id),
        Wait::Question(id) => named("question", *id),
        Wait::Design(id) => named("design", *id),
        Wait::Condition(text) => WaitView {
            kind: "condition",
            id: None,
            title: None,
            text: Some(text.clone()),
            met: false,
            on_person: false,
        },
    }
}

/// The view of open <item>, with <branches> the times of every branch
/// (`derive::branch_times`).
fn item_view(
    lists: &Lists,
    item: &Item,
    branches: &BTreeMap<(String, String), Times>,
) -> Result<ItemView> {
    Ok(ItemView {
        id: item.id,
        title: item.title.clone(),
        area: item.area.clone(),
        details: item.details.clone(),
        r#for: item.r#for,
        state: derive::item_state(lists, item),
        priority: item.priority.clone(),
        behind: derive::behind(lists, item.id),
        brief: item.brief.clone(),
        waits: item
            .waits
            .iter()
            .map(|wait| wait_view(lists, wait))
            .collect(),
        running: item
            .running
            .iter()
            .map(|running| {
                let (repo, checkout) = side_worktree_parts(&running.worktree)
                    .expect("a stored running worktree passed the check on load");
                let times = branches[&(repo.clone(), running.branch.clone())].clone();
                RunningView {
                    worktree: running.worktree.clone(),
                    repo,
                    checkout,
                    branch: running.branch.clone(),
                    brief: running.brief.clone(),
                    since: running.since.clone(),
                    times,
                    claim: None,
                }
            })
            .collect(),
        parked: item.parked.clone(),
        landed: item.landed.clone(),
        times: derive::item_times(lists, item, None)?,
        notes: item.notes.clone(),
        opened: item.opened.clone(),
        trail: trail(lists, item.id)?,
    })
}

fn place_item(lists: &Lists, id: Id) -> PlaceItem {
    match lists.locate(id) {
        Some(Located {
            standing: Standing::Open,
            title,
            ..
        }) => PlaceItem {
            id,
            title: title.to_string(),
            state: derive::item_state(lists, lists.item(id).expect("located as an open item"))
                .label(),
        },
        Some(Located { title, .. }) => PlaceItem {
            id,
            title: title.to_string(),
            state: "closed",
        },
        None => PlaceItem {
            id,
            title: String::new(),
            state: "closed",
        },
    }
}

pub fn design_view(lists: &Lists, design: &DesignChange, closed: Option<Stamp>) -> DesignView {
    DesignView {
        id: design.id,
        title: design.title.clone(),
        details: design.details.clone(),
        repo: design.repo.clone(),
        places: design
            .places
            .iter()
            .map(|(path, resolution)| match resolution {
                Resolution::Item(id) => PlaceView {
                    path: path.clone(),
                    item: Some(place_item(lists, *id)),
                    matches: None,
                },
                Resolution::Matches(how) => PlaceView {
                    path: path.clone(),
                    item: None,
                    matches: Some(how.clone()),
                },
            })
            .collect(),
        deleted: design.deleted.iter().cloned().collect(),
        blocks: lists
            .items
            .iter()
            .filter(|item| item.waits.contains(&Wait::Design(design.id)))
            .map(|item| Named {
                id: item.id,
                title: item.title.clone(),
            })
            .collect(),
        no_app_place: design.no_app_place.clone(),
        approved: design.approved.clone(),
        decision: match &design.settlement {
            crate::lists::design::Settlement::Decision(id) => Some(Named {
                id: *id,
                title: lists
                    .decision(*id)
                    .map(|d| d.title.clone())
                    .unwrap_or_default(),
            }),
            _ => None,
        },
        session: design.session,
        opened: design.opened.clone(),
        landed: design.landed.clone(),
        closed,
    }
}

/// The open design changes, then those that landed in the last `FLOW_DAYS`
/// days.
fn designs(lists: &Lists, now: i64) -> Vec<DesignView> {
    let recent = now - FLOW_DAYS as i64 * 86400;
    let mut views: Vec<DesignView> = lists
        .designs
        .iter()
        .map(|d| design_view(lists, d, None))
        .collect();
    for closed in lists.closed.iter().rev() {
        if let crate::lists::Record::Design(design) = &closed.record
            && matches!(closed.how, crate::lists::How::Landed(_))
            && closed.closed.at >= recent
        {
            views.push(design_view(lists, design, Some(closed.closed.clone())));
        }
    }
    views
}

fn decisions(lists: &Lists) -> Vec<DecisionView> {
    derive::decisions_newest_first(lists)
        .into_iter()
        .map(|decision| DecisionView {
            id: decision.id,
            date: decision.date.clone(),
            title: decision.title.clone(),
            by: decision.by.clone(),
            body: decision.body.clone(),
            question: decision.question,
            work: decision
                .work
                .iter()
                .map(|id| WorkView {
                    id: *id,
                    title: lists
                        .locate(*id)
                        .map(|found| found.title.to_string())
                        .unwrap_or_default(),
                    open: lists.item(*id).is_some(),
                })
                .collect(),
            replaces: decision.replaces,
            replaced_by: derive::replaced_by(lists, decision.id),
            plan: decision.plan.clone(),
        })
        .collect()
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
