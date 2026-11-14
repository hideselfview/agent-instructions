//! What is derived, never stored: met waits, an item's state, the stack's
//! order and what each entry blocks, dispatch order, and flow. Shared by the
//! commands and the daemon.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::lists::{
    Audience, Closed, Decision, Id, Item, Kind, Level, Lists, Located, Position, Question, Record,
    Standing, Wait, day,
};
use crate::text::local_day;

/// How many days back flow counts, today included.
pub const FLOW_DAYS: usize = 14;

/// Does a record that closed this way meet the waits on it? A question
/// answered, settled or done; an item or design change landed. One that was
/// dropped, deferred or moved to the stack never does.
pub fn closed_meets_wait(closed: &Closed) -> bool {
    use crate::lists::How;
    matches!(
        (&closed.record, &closed.how),
        (
            Record::Question(_),
            How::Answered(_) | How::Settled(_) | How::Done
        ) | (Record::Item(_) | Record::Design(_), How::Landed(_))
    )
}

/// The decisions, newest first: by date, then by id.
pub fn decisions_newest_first(lists: &Lists) -> Vec<&Decision> {
    let mut ordered: Vec<&Decision> = lists.decisions.iter().collect();
    ordered.sort_by(|a, b| (&b.date, b.id).cmp(&(&a.date, a.id)));
    ordered
}

/// The newest decision that replaces <decision>. Derived, never stored.
pub fn replaced_by(lists: &Lists, decision: Id) -> Option<Id> {
    lists
        .decisions
        .iter()
        .filter(|other| other.replaces == Some(decision))
        .map(|other| other.id)
        .max()
}

/// Is <wait> met? A wait on a record that does not exist is not (the check
/// refuses such a store); a condition is met only by being removed, so a
/// condition still there is not.
pub fn wait_met(lists: &Lists, wait: &Wait) -> bool {
    let id = match wait {
        Wait::Item(id) | Wait::Question(id) | Wait::Design(id) => *id,
        Wait::Condition(_) => return false,
    };
    match lists.locate(id) {
        Some(Located {
            standing: Standing::Closed(closed),
            ..
        }) => closed_meets_wait(closed),
        _ => false,
    }
}

/// The waits of <item> not met.
pub fn unmet_waits<'a>(lists: &Lists, item: &'a Item) -> Vec<&'a Wait> {
    item.waits
        .iter()
        .filter(|wait| !wait_met(lists, wait))
        .collect()
}

/// An item `last` in priority waits while any other open coordinator item
/// that is not itself `last` exists.
pub fn waits_for_others(lists: &Lists, item: &Item) -> bool {
    item.priority
        .as_ref()
        .is_some_and(|p| p.level == Level::Last)
        && lists.items.iter().any(|other| {
            other.id != item.id
                && other.r#for == Audience::Coordinator
                && !other
                    .priority
                    .as_ref()
                    .is_some_and(|p| p.level == Level::Last)
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemState {
    /// An agent works on it (it may also wait, when its agent is held on an
    /// answer).
    Running,
    /// The design session's, shown apart and never dispatchable.
    DesignSession,
    Waiting,
    Ready,
}

impl ItemState {
    pub fn label(self) -> &'static str {
        match self {
            ItemState::Running => "running",
            ItemState::DesignSession => "design-session",
            ItemState::Waiting => "waiting",
            ItemState::Ready => "ready",
        }
    }
}

pub fn item_state(lists: &Lists, item: &Item) -> ItemState {
    if !item.running.is_empty() {
        ItemState::Running
    } else if item.r#for == Audience::DesignSession {
        ItemState::DesignSession
    } else if !unmet_waits(lists, item).is_empty() || waits_for_others(lists, item) {
        ItemState::Waiting
    } else {
        ItemState::Ready
    }
}

/// An entry of the stack with what it blocks.
#[derive(Clone, Debug)]
pub struct StackEntry<'a> {
    pub question: &'a Question,
    /// The open items that wait on it directly, in queue order.
    pub blocks: Vec<&'a Item>,
    /// The open items that wait on it directly or through a chain of open
    /// items.
    pub blocked_count: usize,
}

fn blocked_through_chains(lists: &Lists, question: &Question) -> (Vec<Id>, BTreeSet<Id>) {
    let direct: Vec<Id> = lists
        .items
        .iter()
        .filter(|item| item.waits.contains(&Wait::Question(question.id)))
        .map(|item| item.id)
        .collect();
    let mut all: BTreeSet<Id> = direct.iter().copied().collect();
    let mut frontier = direct.clone();
    while let Some(id) = frontier.pop() {
        for item in &lists.items {
            if item.waits.contains(&Wait::Item(id)) && all.insert(item.id) {
                frontier.push(item.id);
            }
        }
    }
    (direct, all)
}

/// The stack in the order it is asked: `placed first` (newest first), then by
/// how many open items wait on each directly or through a chain of open
/// items, most first, ties oldest first, then `placed last` (by the same
/// count and age). The first is the question being asked now.
pub fn stack_order(lists: &Lists) -> Vec<StackEntry<'_>> {
    let mut entries: Vec<StackEntry<'_>> = lists
        .questions
        .iter()
        .map(|question| {
            let (direct, all) = blocked_through_chains(lists, question);
            StackEntry {
                question,
                blocks: direct.iter().filter_map(|id| lists.item(*id)).collect(),
                blocked_count: all.len(),
            }
        })
        .collect();
    let group = |entry: &StackEntry<'_>| match entry.question.placed.as_ref().map(|p| p.at) {
        Some(Position::First) => 0,
        None => 1,
        Some(Position::Last) => 2,
    };
    entries.sort_by(|a, b| {
        group(a).cmp(&group(b)).then_with(|| {
            if group(a) == 0 {
                (b.question.opened.at, b.question.id).cmp(&(a.question.opened.at, a.question.id))
            } else {
                b.blocked_count.cmp(&a.blocked_count).then_with(|| {
                    (a.question.opened.at, a.question.id)
                        .cmp(&(b.question.opened.at, b.question.id))
                })
            }
        })
    });
    entries
}

/// The ready coordinator items in dispatch order: `priority first` (oldest
/// first), then the rest by area order and age. `last` items are ready only
/// once nothing else is open, so they come out last by themselves.
pub fn dispatch_order(lists: &Lists) -> Vec<&Item> {
    let area_rank = |item: &Item| {
        lists
            .areas
            .iter()
            .position(|a| *a == item.area)
            .unwrap_or(usize::MAX)
    };
    let level = |item: &Item| item.priority.as_ref().map(|p| p.level);
    let group = |item: &Item| match level(item) {
        Some(Level::First) => 0,
        None => 1,
        Some(Level::Last) => 2,
    };
    let mut ready: Vec<&Item> = lists
        .items
        .iter()
        .filter(|item| {
            item.r#for == Audience::Coordinator && item_state(lists, item) == ItemState::Ready
        })
        .collect();
    ready.sort_by(|a, b| {
        group(a).cmp(&group(b)).then_with(|| {
            let by_age = (a.opened.at, a.id).cmp(&(b.opened.at, b.id));
            if group(a) == 1 {
                area_rank(a).cmp(&area_rank(b)).then(by_age)
            } else {
                by_age
            }
        })
    });
    ready
}

/// One day of flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FlowDay {
    pub day: String,
    /// Items and questions opened, by who.
    pub opened: BTreeMap<String, u32>,
    /// Items and questions that left, by how.
    pub closed: BTreeMap<String, u32>,
}

/// The last `FLOW_DAYS` days ending with the day of <now>, newest first, from
/// the opened and closed stamps.
pub fn flow(lists: &Lists, now: i64) -> Vec<FlowDay> {
    let today = local_day(now);
    let mut days: Vec<FlowDay> = (0..FLOW_DAYS as i64)
        .map(|back| FlowDay {
            day: day::days_before(&today, back).expect("local_day gives a valid date"),
            opened: BTreeMap::new(),
            closed: BTreeMap::new(),
        })
        .collect();
    let mut opened = |at: i64, by: &str| {
        let day = local_day(at);
        if let Some(entry) = days.iter_mut().find(|d| d.day == day) {
            *entry.opened.entry(by.to_string()).or_insert(0) += 1;
        }
    };
    for question in &lists.questions {
        opened(question.opened.at, &question.opened.by);
    }
    for item in &lists.items {
        opened(item.opened.at, &item.opened.by);
    }
    for closed in &lists.closed {
        if closed.record.kind() != Kind::Design {
            opened(closed.record.opened().at, &closed.record.opened().by);
        }
    }
    for closed in lists
        .closed
        .iter()
        .filter(|c| c.record.kind() != Kind::Design)
    {
        let day = local_day(closed.closed.at);
        if let Some(entry) = days.iter_mut().find(|d| d.day == day) {
            *entry
                .closed
                .entry(closed.how.label().to_string())
                .or_insert(0) += 1;
        }
    }
    days
}

#[cfg(test)]
#[path = "derive_tests.rs"]
mod tests;
