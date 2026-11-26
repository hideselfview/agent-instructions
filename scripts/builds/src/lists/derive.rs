//! What is derived, never stored: met waits, an item's state, the stack's
//! order and what each entry blocks, dispatch order, and what the runs
//! of an item or a branch add up to. The commands and the daemon both use
//! these functions.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::fail;
use crate::fail::Result;
use crate::lists::{
    Audience, Closed, Decision, DesignChange, Ended, How, Id, Item, Level, Lists, Located,
    Position, Question, Record, Span, SpanKind, Standing, Wait, side_worktree_parts,
};

/// How many days back a recently landed item or design change stays shown.
pub const FLOW_DAYS: usize = 14;

/// Does a record that closed this way free the items that wait on it? A
/// question frees them when someone answers, settles or completes it. An item
/// or design change frees them when it lands, and an item also frees them
/// when it finishes. A record that someone dropped or moved to the stack
/// never frees them.
pub fn closed_meets_wait(closed: &Closed) -> bool {
    use crate::lists::How;
    matches!(
        (&closed.record, &closed.how),
        (
            Record::Question(_),
            How::Answered(_) | How::Settled(_) | How::Done(_) | How::DoneUnrecorded
        ) | (Record::Item(_) | Record::Design(_), How::Landed(_))
            | (Record::Item(_), How::Finished(_))
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
        Wait::Condition { .. } => return false,
    };
    match lists.locate(id) {
        Some(Located {
            standing: Standing::Closed(closed),
            ..
        }) => closed_meets_wait(closed),
        _ => false,
    }
}

/// Does <wait> wait on the person? A wait on an open question waits on their
/// answer, and a wait on an open design change waits on their review until
/// they approve it; after that, it waits only on the change's landing. A wait
/// on an item or a condition is never theirs.
pub fn wait_on_person(lists: &Lists, wait: &Wait) -> bool {
    match wait {
        Wait::Question(_) => !wait_met(lists, wait),
        Wait::Design(id) => lists.design(*id).is_some_and(DesignChange::awaits_approval),
        Wait::Item(_) | Wait::Condition { .. } => false,
    }
}

/// The waits of <item> not met.
pub fn unmet_waits<'a>(lists: &Lists, item: &'a Item) -> Vec<&'a Wait> {
    item.waits
        .iter()
        .filter(|wait| !wait_met(lists, wait))
        .collect()
}

/// An item `last` in priority waits while any other open item for the same
/// audience that is not itself `last` exists.
pub fn waits_for_others(lists: &Lists, item: &Item) -> bool {
    items_ahead_of_last(lists, item).next().is_some()
}

/// The open items whose priority keeps a last-place item from starting.
pub fn items_ahead_of_last<'a>(lists: &'a Lists, item: &'a Item) -> impl Iterator<Item = &'a Item> {
    let candidates = if item.running.is_empty()
        && item
            .priority
            .as_ref()
            .is_some_and(|p| p.level == Level::Last)
    {
        lists.items.as_slice()
    } else {
        &[]
    };
    candidates.iter().filter(move |other| {
        other.id != item.id
            && other.r#for == item.r#for
            && other.backlog.is_none()
            && !other
                .priority
                .as_ref()
                .is_some_and(|p| p.level == Level::Last)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemState {
    /// An agent works on it (it may also wait, when its agent is held on an
    /// answer).
    Running,
    /// Waits until the person starts it; no agent takes it.
    Backlog,
    /// The design session's, shown apart and never dispatchable.
    DesignSession,
    Waiting,
    Ready,
}

impl ItemState {
    pub fn label(self) -> &'static str {
        match self {
            ItemState::Running => "running",
            ItemState::Backlog => "backlog",
            ItemState::DesignSession => "design-session",
            ItemState::Waiting => "waiting",
            ItemState::Ready => "ready",
        }
    }
}

pub fn item_state(lists: &Lists, item: &Item) -> ItemState {
    if !item.running.is_empty() {
        ItemState::Running
    } else if item.backlog.is_some() {
        ItemState::Backlog
    } else if item.r#for == Audience::DesignSession {
        ItemState::DesignSession
    } else if !can_start(lists, item) {
        ItemState::Waiting
    } else {
        ItemState::Ready
    }
}

/// Can <item> start now: no agent runs it, it is not in the backlog, every
/// wait is met, and it is not held back by being placed last.
fn can_start(lists: &Lists, item: &Item) -> bool {
    item.running.is_empty()
        && item.backlog.is_none()
        && unmet_waits(lists, item).is_empty()
        && !waits_for_others(lists, item)
}

/// An entry of the stack with what it blocks.
#[derive(Clone, Debug)]
pub struct StackEntry<'a> {
    pub question: &'a Question,
    /// The open items that wait on it directly, in queue order.
    pub blocks: Vec<BlockedItem<'a>>,
    /// The open items that wait on it directly or through a chain of open
    /// items.
    pub blocked_count: usize,
}

/// An open item that waits on a question directly.
#[derive(Clone, Debug)]
pub struct BlockedItem<'a> {
    pub item: &'a Item,
    /// How many open items wait behind it: on it directly or through a chain
    /// of open items.
    pub behind: usize,
}

/// The open items that wait on any of <ids> directly or through a chain of
/// open items.
fn waiting_behind(lists: &Lists, ids: &[Id]) -> BTreeSet<Id> {
    let mut all = BTreeSet::new();
    let mut frontier = ids.to_vec();
    while let Some(id) = frontier.pop() {
        for item in &lists.items {
            if item.backlog.is_none() && item.waits.contains(&Wait::Item(id)) && all.insert(item.id)
            {
                frontier.push(item.id);
            }
        }
    }
    all
}

/// How many open items wait behind item <id>: on it directly or through a
/// chain of open items.
pub fn behind(lists: &Lists, id: Id) -> usize {
    waiting_behind(lists, &[id]).len()
}

/// The stack in the order in which builds asks the person its questions: the
/// entries placed first in the person's ranked order, highest rank on top;
/// then by how many open items wait on each directly or through a chain of
/// open items, most first, then by effect in `Effect`'s order, then oldest
/// first; then `placed last` (by the same count, effect and age). The first
/// entry is the question builds asks now.
pub fn stack_order(lists: &Lists) -> Vec<StackEntry<'_>> {
    let mut entries: Vec<StackEntry<'_>> = lists
        .questions
        .iter()
        .map(|question| {
            let asked = Wait::Question(question.id);
            let direct: Vec<&Item> = lists.open_items_waiting_on(&asked).collect();
            let ids: Vec<Id> = direct.iter().map(|item| item.id).collect();
            let mut all = waiting_behind(lists, &ids);
            all.extend(&ids);
            StackEntry {
                question,
                blocks: direct
                    .into_iter()
                    .map(|item| BlockedItem {
                        item,
                        behind: behind(lists, item.id),
                    })
                    .collect(),
                blocked_count: all.len(),
            }
        })
        .collect();
    entries.sort_by_key(|entry| {
        // Ranks are unique, so the count, effect and age order only the
        // entries outside the ranked order, which all take rank 0 here.
        let (group, rank) = match entry.question.placed.as_ref().map(|p| p.at) {
            Some(Position::First { rank }) => (0, rank),
            None => (1, 0),
            Some(Position::Last) => (2, 0),
        };
        (
            group,
            Reverse(rank),
            Reverse(entry.blocked_count),
            entry.question.effect,
            entry.question.opened.at,
            entry.question.id,
        )
    });
    entries
}

/// Sorts <items> into the queue's order, the one `builds next` and the
/// dashboard's Queue tab show: `priority first` items at the top (oldest
/// first), then the rest by how many open items wait on each directly or
/// through a chain of open items (most first, as the stack counts them), then
/// by area order and age, then `priority last` items (oldest first).
pub fn queue_order(lists: &Lists, items: &mut [&Item]) {
    let area_rank = |item: &Item| {
        lists
            .areas
            .iter()
            .position(|a| *a == item.area)
            .unwrap_or(usize::MAX)
    };
    let group = |item: &Item| match item.priority.as_ref().map(|p| p.level) {
        Some(Level::First) => 0,
        None => 1,
        Some(Level::Last) => 2,
    };
    let unblocked: BTreeMap<Id, usize> = items
        .iter()
        .map(|item| (item.id, behind(lists, item.id)))
        .collect();
    items.sort_by(|a, b| {
        group(a).cmp(&group(b)).then_with(|| {
            let by_age = (a.opened.at, a.id).cmp(&(b.opened.at, b.id));
            if group(a) == 1 {
                unblocked[&b.id]
                    .cmp(&unblocked[&a.id])
                    .then(area_rank(a).cmp(&area_rank(b)))
                    .then(by_age)
            } else {
                by_age
            }
        })
    });
}

/// The items for <audience> that can start now, in dispatch order
/// (`queue_order`). `last` items come out only once no other open item for the
/// audience remains, so they come out last by themselves.
pub fn dispatch_order(lists: &Lists, audience: Audience) -> Vec<&Item> {
    let mut ready: Vec<&Item> = lists
        .items
        .iter()
        .filter(|item| item.r#for == audience && can_start(lists, item))
        .collect();
    queue_order(lists, &mut ready);
    ready
}

/// A time that adds up spans: the seconds of the spans that ended, and the
/// start of the span that goes on, if one does. At the moment `now` the time
/// is `seconds + (now - since)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Total {
    pub seconds: i64,
    pub since: Option<i64>,
}

impl Total {
    /// The time that <spans> cover, a moment that two of them cover counted
    /// once. Each span is its start and its end, None while it goes on.
    pub fn of(spans: impl IntoIterator<Item = (i64, Option<i64>)>) -> Total {
        let mut ended = Vec::new();
        let mut since: Option<i64> = None;
        for (start, end) in spans {
            match end {
                Some(end) => ended.push((start, end)),
                None => since = Some(since.map_or(start, |open| open.min(start))),
            }
        }
        ended.sort();
        let mut seconds = 0;
        let mut reached = i64::MIN;
        for (start, end) in ended {
            // The span that goes on counts the time from its start.
            let end = since.map_or(end, |open| end.min(open));
            let start = start.max(reached);
            if end > start {
                seconds += end - start;
            }
            reached = reached.max(end);
        }
        Total { seconds, since }
    }

    /// The seconds at <now>.
    pub fn at(&self, now: i64) -> i64 {
        self.seconds + self.since.map_or(0, |since| (now - since).max(0))
    }
}

/// What runs add up to: the runs of one item, or of every item that ran on
/// one branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Times {
    /// When the first run started.
    pub first: i64,
    /// How many runs, the one that goes on included.
    pub runs: usize,
    pub ran: Total,
    /// The time the worktree's claim note carried the state held.
    pub held: Total,
    /// The time an item waited on a question or on a design change the
    /// person had not approved.
    pub waiting: Total,
    /// When the work landed: the end of the last run, when that run ended
    /// with its item's landing and no run goes on.
    pub landed: Option<i64>,
    /// When the store began to record spans, if that was after the first run
    /// started: `held` and `waiting` leave out the time before it.
    pub spans_from: Option<i64>,
}

/// A run as `Times` adds it up: its start, its end (None while it goes on),
/// whether its item's landing ended it, the repo and branch of each place it
/// ran in, and its item's spans that began within it.
struct Ran<'a> {
    since: i64,
    ended: Option<i64>,
    landed: bool,
    branches: Vec<(String, String)>,
    spans: Vec<&'a Span>,
}

/// The runs of <item>, ended ones first, then the one that goes on. <closed>
/// is how the item closed, None while it is open. Fails naming a span that
/// lies in none of the item's runs: builds opens a span only while its item
/// runs and ends it no later than the run.
fn runs_of<'a>(item: &'a Item, closed: Option<&How>) -> Result<Vec<Ran<'a>>> {
    let landing = matches!(closed, Some(How::Landed(_)));
    let mut runs: Vec<Ran> = item
        .runs
        .iter()
        .map(|run| Ran {
            since: run.since.at,
            ended: Some(run.ended.at),
            landed: landing && run.how == Ended::Closed,
            branches: run
                .places
                .iter()
                .map(|place| (place.repo.clone(), place.branch.clone()))
                .collect(),
            spans: Vec::new(),
        })
        .collect();
    if let Some(first) = item.running.first() {
        runs.push(Ran {
            since: first.since.at,
            ended: None,
            landed: false,
            branches: item
                .running
                .iter()
                .filter_map(|running| {
                    side_worktree_parts(&running.worktree)
                        .map(|(repo, _)| (repo, running.branch.clone()))
                })
                .collect(),
            spans: Vec::new(),
        });
    }
    for span in &item.spans {
        let within = runs.iter_mut().find(|run| {
            run.since <= span.since
                && match (run.ended, span.ended) {
                    (None, _) => true,
                    (Some(run_end), Some(span_end)) => span_end <= run_end,
                    (Some(_), None) => false,
                }
        });
        match within {
            Some(run) => run.spans.push(span),
            None => fail!(
                "item {} has a {} span from {} that lies in none of its runs",
                item.id,
                span.kind.word(),
                span.since
            ),
        }
    }
    Ok(runs)
}

/// What <runs> add up to, with <spans_from> the store's
/// (`Lists::spans_from`). None when there are no runs.
fn times_of(runs: &[&Ran], spans_from: Option<i64>) -> Option<Times> {
    let first = runs.iter().map(|run| run.since).min()?;
    let spans = |kind: SpanKind| {
        Total::of(
            runs.iter()
                .flat_map(|run| run.spans.iter())
                .filter(|span| span.kind == kind)
                .map(|span| (span.since, span.ended)),
        )
    };
    let goes_on = runs.iter().any(|run| run.ended.is_none());
    let last = runs.iter().max_by_key(|run| run.ended);
    Some(Times {
        first,
        runs: runs.len(),
        ran: Total::of(runs.iter().map(|run| (run.since, run.ended))),
        held: spans(SpanKind::Held),
        waiting: spans(SpanKind::Waiting),
        landed: last
            .filter(|run| !goes_on && run.landed)
            .and_then(|run| run.ended),
        spans_from: spans_from.filter(|from| *from > first),
    })
}

/// What the runs of <item> add up to; None when it never ran. <closed> is
/// how it closed, None while it is open. Fails as `runs_of` does.
pub fn item_times(lists: &Lists, item: &Item, closed: Option<&How>) -> Result<Option<Times>> {
    let runs = runs_of(item, closed)?;
    Ok(times_of(&runs.iter().collect::<Vec<_>>(), lists.spans_from))
}

/// What the runs on each branch add up to, by repo and branch: the runs of
/// every item, open or closed, in a worktree of the repo on the branch. A
/// run that a store move read from the write log names no branch, and no
/// branch counts it. Fails as `runs_of` does.
pub fn branch_times(lists: &Lists) -> Result<BTreeMap<(String, String), Times>> {
    let open = lists.items.iter().map(|item| (item, None));
    let closed = lists
        .closed
        .iter()
        .filter_map(|closed| match &closed.record {
            Record::Item(item) => Some((item, Some(&closed.how))),
            _ => None,
        });
    let mut runs: Vec<Ran> = Vec::new();
    for (item, how) in open.chain(closed) {
        runs.extend(runs_of(item, how)?);
    }
    let mut by_branch: BTreeMap<(String, String), Vec<&Ran>> = BTreeMap::new();
    for run in &runs {
        for branch in &run.branches {
            by_branch.entry(branch.clone()).or_default().push(run);
        }
    }
    Ok(by_branch
        .into_iter()
        .filter_map(|(branch, runs)| Some((branch, times_of(&runs, lists.spans_from)?)))
        .collect())
}

#[cfg(test)]
#[path = "derive_tests.rs"]
mod tests;
