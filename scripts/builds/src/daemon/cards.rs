//! The words on a worktree's card: the chip in its top bar, the pill that
//! names what the card waits on, the running items it lists, its note (when
//! no item runs, or what a held worktree waits on), and its uncommitted
//! files. The page draws them; the times and the commits come from the
//! checkout's own facts.

use super::model::CheckoutPage;
use crate::lane::Lane;
use crate::lists::Ender;

/// What a running item waits on that is not met.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemWait {
    /// An open question for the person.
    Answer { id: u32, title: String },
    /// A design change that waits on the person's review.
    Review { id: u32, title: String },
    /// An app build of the branch runs or waits in <lane>: `running` says
    /// whether it builds now and `how` says how far it is.
    AppBuild {
        lane: Lane,
        running: bool,
        how: String,
        /// Its place in the lane's queue, when it has not started.
        queued: Option<QueuedBuild>,
    },
    /// Another item.
    Item {
        id: u32,
        title: String,
        /// The item's state, or how it closed without freeing this one.
        state: &'static str,
    },
    /// A design change the person approved, which has not landed.
    Landing { id: u32, title: String },
    /// Commits of a design branch that no recorded design change covers.
    Unrecorded,
    /// Commits waiting to land whose rules review `builds land` would not
    /// take yet (review_cards.rs).
    RulesReview(ReviewWait),
    /// A condition in words, which holds until its ender removes it from the
    /// item.
    Condition { text: String, until: Ender },
}

/// Where the rules review of a branch stands while `builds land` would refuse
/// its commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewWait {
    /// No review read these commits, or a group still has to answer over
    /// them: the branch's agent runs the review.
    NotRun,
    /// A review of the branch's newest commit runs now.
    Running,
    /// This many findings wait for the settler to settle them.
    ToSettle(usize, Settler),
    /// This many confirmed findings are open: they wait for a review that
    /// closes them.
    Open(usize),
    /// This many files changed since the last review: `builds review` reads
    /// those changes in a new run.
    Unreviewed(usize),
}

/// Who settles a review's findings: the judge while one listens
/// (review/judge.rs), else the coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settler {
    Judge,
    Coordinator,
}

impl ReviewWait {
    /// The card's chip.
    fn chip(self) -> &'static str {
        match self {
            ReviewWait::NotRun => "Needs review",
            ReviewWait::Running => "Reviewing",
            ReviewWait::ToSettle(..) => "Findings to settle",
            ReviewWait::Open(_) => "Open findings",
            ReviewWait::Unreviewed(_) => "Unreviewed changes",
        }
    }

    /// The pill in the card's bar; None when the chip already names the wait.
    fn pill(self) -> Option<Pill> {
        let text = match self {
            ReviewWait::NotRun | ReviewWait::Running => return None,
            ReviewWait::ToSettle(n, _) | ReviewWait::Open(n) => {
                crate::text::counted(n, "finding", "findings")
            }
            ReviewWait::Unreviewed(n) => crate::text::counted(n, "file", "files"),
        };
        Some(Pill {
            text,
            title: self.sentence(),
        })
    }

    /// Where the review stands, in a sentence.
    fn sentence(self) -> String {
        let findings = |n: usize, what: &str| match n {
            1 => format!("1 {what} finding waits"),
            n => format!("{n} {what} findings wait"),
        };
        match self {
            // The chip says it.
            ReviewWait::NotRun => String::new(),
            ReviewWait::Running => "The rules review of its commits runs.".to_string(),
            ReviewWait::ToSettle(n, settler) => format!(
                "{} on the {} to settle {}.",
                findings(n, "review"),
                match settler {
                    Settler::Judge => "judge",
                    Settler::Coordinator => "coordinator",
                },
                if n == 1 { "it" } else { "them" }
            ),
            ReviewWait::Open(n) => format!(
                "{} for a review that closes {}.",
                findings(n, "open confirmed review"),
                if n == 1 { "it" } else { "them" }
            ),
            ReviewWait::Unreviewed(1) => "1 file changed since the last review: builds review reads the changes in a new run.".to_string(),
            ReviewWait::Unreviewed(n) => format!(
                "{n} files changed since the last review: builds review reads the changes in a new run."
            ),
        }
    }
}

/// <lane>'s name as a card writes it.
fn lane_title(lane: Lane) -> &'static str {
    match lane {
        Lane::Mac => "Mac",
        Lane::Android => "Android",
    }
}

/// A build that waits in a lane's queue.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct QueuedBuild {
    /// Its place, 1 for the next build to start.
    pub place: usize,
    /// How many builds wait in the lane.
    pub of: usize,
    /// "Its agent asked 8m ago to run macos macos-tests."
    pub line: String,
    /// The steps in `line`, which the page draws in the mono face.
    pub steps: Option<String>,
}

impl ItemWait {
    /// What the person has to do comes before another item. A condition comes
    /// after every other wait: the daemon sets a card's state from its first
    /// wait, and a condition sets no state.
    pub fn rank(&self) -> u8 {
        match self {
            ItemWait::Answer { .. } => 0,
            ItemWait::Review { .. } => 1,
            ItemWait::AppBuild { .. } => 2,
            ItemWait::Item { .. } | ItemWait::Landing { .. } => 3,
            ItemWait::Unrecorded => 4,
            ItemWait::RulesReview(_) => 5,
            ItemWait::Condition { .. } => 6,
        }
    }

    /// Does only the person's answer or review end this wait?
    pub fn needs_person(&self) -> bool {
        matches!(self, ItemWait::Answer { .. } | ItemWait::Review { .. })
    }

    /// The pill in the card's bar; None for an app build that has not
    /// started and a rules review that has no findings yet, which the chip
    /// names, and for a condition, whose words are a sentence.
    fn pill(&self) -> Option<Pill> {
        let text = match self {
            ItemWait::Answer { id, .. } => format!("Answer #{id}"),
            ItemWait::Review { id, .. } => format!("Review #{id}"),
            ItemWait::AppBuild {
                lane,
                running: true,
                ..
            } => format!("{} build", lane_title(*lane)),
            ItemWait::AppBuild { running: false, .. } => return None,
            ItemWait::Item { id, .. } | ItemWait::Landing { id, .. } => format!("After #{id}"),
            ItemWait::Unrecorded => "No design change".to_string(),
            ItemWait::RulesReview(review) => return review.pill(),
            ItemWait::Condition { .. } => return None,
        };
        Some(Pill {
            text,
            title: self.sentence(),
        })
    }

    /// The wait in a sentence.
    fn sentence(&self) -> String {
        match self {
            ItemWait::Answer { id, title } => format!("Waits on your answer to #{id}: {title}"),
            ItemWait::Review { id, title } => {
                format!("Waits on your review of design change {id}: {title}")
            }
            ItemWait::AppBuild {
                lane,
                running: true,
                how,
                ..
            } => format!("{} app build: {how}", lane_title(*lane)),
            ItemWait::AppBuild { lane, how, .. } => {
                format!("Waits on the {} app build: {how}", lane_title(*lane))
            }
            ItemWait::Item { id, title, state } => {
                format!("Waits on #{id} ({state}): {title}")
            }
            ItemWait::Landing { id, title } => {
                format!("Waits on design change {id} to land: {title}")
            }
            ItemWait::Unrecorded => {
                "Waits until a recorded design change covers the design files its commits change."
                    .to_string()
            }
            ItemWait::RulesReview(review) => review.sentence(),
            ItemWait::Condition { text, until } => {
                format!("Waits until {text}; the {} ends this wait", until.word())
            }
        }
    }
}

/// Returns <waits> in rank order, each wait once; waits of one rank keep the
/// order <waits> gave them.
pub fn ranked(waits: impl IntoIterator<Item = ItemWait>) -> Vec<ItemWait> {
    let mut ranked: Vec<ItemWait> = Vec::new();
    for wait in waits {
        if !ranked.contains(&wait) {
            ranked.push(wait);
        }
    }
    ranked.sort_by_key(ItemWait::rank);
    ranked
}

/// The state a side worktree's card shows, first match wins in `card_state`.
/// Its chip names it, the page groups and orders cards by its `word`, and
/// builds records each change of its `name` (card_state.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    /// Unclaimed, nothing waiting to land.
    Free,
    /// Claimed, and none of the states below.
    Working,
    /// Its claim note says winding-down: the agent's last item, then its
    /// handoff.
    WindingDown,
    /// A running item's app build waits in this lane's queue.
    Queued(Lane),
    /// The claim note says so, or the first wait is one only the person ends.
    WaitingOnYou,
    /// The claim note says held: kept on purpose with no work running.
    Paused,
    /// Commits wait to land, and nothing else goes on.
    ReadyToLand,
    /// Commits wait to land and their rules review would keep `builds land`
    /// from taking them.
    InReview(ReviewWait),
    /// The daemon could not read what its running items wait on.
    WaitsUnread,
    /// An open item keeps the worktree with no agent, and has no waits.
    Kept,
    /// An open item keeps the worktree with no agent, and waits on an open
    /// record.
    KeptWaiting,
    /// An open item keeps the worktree with no agent, and every wait is met.
    KeptMet,
}

impl CardState {
    /// The group the page sorts and colors the card by.
    pub fn word(self) -> &'static str {
        match self {
            CardState::Free => "free",
            CardState::Working => "working",
            CardState::WindingDown => "winding-down",
            CardState::Queued(_) => "queued",
            CardState::WaitingOnYou => "waiting-on-you",
            CardState::Paused => "held",
            CardState::ReadyToLand => "ready-to-land",
            CardState::InReview(_) => "in-review",
            CardState::WaitsUnread => "waits-unread",
            CardState::Kept => "kept",
            CardState::KeptWaiting => "kept-waiting",
            CardState::KeptMet => "kept-met",
        }
    }

    /// The state's name, as builds records it: one per chip, and the review's
    /// counts left out.
    pub fn name(self) -> &'static str {
        match self {
            CardState::Queued(Lane::Mac) => "waiting-on-mac",
            CardState::Queued(Lane::Android) => "waiting-on-android",
            CardState::Paused => "paused",
            CardState::InReview(ReviewWait::NotRun) => "needs-review",
            CardState::InReview(ReviewWait::Running) => "reviewing",
            CardState::InReview(ReviewWait::ToSettle(..)) => "findings-to-settle",
            CardState::InReview(ReviewWait::Open(_)) => "open-findings",
            CardState::InReview(ReviewWait::Unreviewed(_)) => "unreviewed-changes",
            other => other.word(),
        }
    }

    /// The chip at the left of the card's top bar; None for a free worktree.
    pub fn chip(self) -> Option<&'static str> {
        Some(match self {
            CardState::Free => return None,
            CardState::Working | CardState::WindingDown => "Working",
            CardState::Queued(Lane::Mac) => "Waiting on Mac",
            CardState::Queued(Lane::Android) => "Waiting on Android",
            CardState::WaitingOnYou => "Waiting on you",
            CardState::Paused => "Paused",
            CardState::ReadyToLand => "Ready to land",
            CardState::InReview(review) => review.chip(),
            CardState::WaitsUnread => "Can't read what this worktree waits on",
            CardState::Kept => "Ready",
            CardState::KeptWaiting => "Waiting",
            CardState::KeptMet => "Waits met",
        })
    }
}

/// The state of a live side worktree's card. <worktree> is what its claim
/// and commits say (model.rs worktree_state, or Needs review when the
/// daemon could not read the rules review of its commits), <shell> says
/// whether an agent shell is active in it, <waits> is what it waits on as
/// `ranked` orders it, or None when the daemon could not read what its
/// running items wait on, and <kept> is what the waits of the item that keeps
/// the worktree with no agent stand at, or None when no item keeps it that
/// way (Err when the daemon could not read them). The dashboard and the
/// recorder of the states (card_recorder.rs) both reach this through
/// `Model::judge_card`.
pub fn card_state(
    worktree: CardState,
    shell: bool,
    waits: Option<&[ItemWait]>,
    kept: Option<Result<&KeptCard, ()>>,
) -> CardState {
    match kept {
        Some(Ok(card)) => return kept_state(card),
        Some(Err(())) => return CardState::WaitsUnread,
        None => {}
    }
    let Some(waits) = waits else {
        return CardState::WaitsUnread;
    };
    let wait = waits.first();
    if wait.is_some_and(ItemWait::needs_person) {
        return CardState::WaitingOnYou;
    }
    let mut state = worktree;
    if keeps_working(state, shell, wait) {
        state = CardState::Working;
    }
    if let (CardState::ReadyToLand, Some(ItemWait::RulesReview(review))) = (state, wait) {
        state = CardState::InReview(*review);
    }
    if let Some(lane) = waits_in_queue(state, wait) {
        state = CardState::Queued(lane);
    }
    state
}

/// Does a card in <state> stay Working instead of Ready to land? Ready to
/// land means commits past origin/main and nothing else going on: no agent
/// shell active in the worktree (<shell>), no app build of its branch and no
/// running item that waits on another item or on a design change to land
/// (<wait>).
fn keeps_working(state: CardState, shell: bool, wait: Option<&ItemWait>) -> bool {
    state == CardState::ReadyToLand
        && (shell
            || matches!(
                wait,
                Some(ItemWait::AppBuild { .. } | ItemWait::Item { .. } | ItemWait::Landing { .. })
            ))
}

/// The lane a Working card in <state> waits on, when its running item's app
/// build waits in that lane's queue and has not started; the card says so in
/// place of Working.
fn waits_in_queue(state: CardState, wait: Option<&ItemWait>) -> Option<Lane> {
    match (state, wait) {
        (
            CardState::Working,
            Some(ItemWait::AppBuild {
                running: false,
                lane,
                ..
            }),
        ) => Some(*lane),
        _ => None,
    }
}

/// A running item, as its card lists it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RunningItem {
    pub id: u32,
    pub title: String,
}

/// What a card waits on, in one or two words; `title` is the whole
/// sentence, which the page shows on hover.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Pill {
    pub text: String,
    pub title: String,
}

/// What a card says in words.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Words {
    /// The chip at the left of the top bar: only a worktree an agent holds
    /// or commits wait in has one.
    pub chip: Option<&'static str>,
    /// What the worktree waits on, at the right of the top bar.
    pub pill: Option<Pill>,
    /// The items running here, in list order, or the item that keeps the worktree.
    pub items: Vec<RunningItem>,
    /// For a held worktree, beside its items: what it waits on, else its
    /// claim note, else what holding means. For any other worktree, only
    /// when no item runs: the claim note, unless the pill's title holds it;
    /// with no claim note and no pill, what the worktree waits on or what
    /// its state means.
    pub note: Option<String>,
    /// The uncommitted files; None for a clean tree, or when the daemon did
    /// not read git here.
    pub uncommitted: Option<Uncommitted>,
    /// The place in a lane's queue of a worktree that waits for its app
    /// build to start.
    pub queued: Option<QueuedBuild>,
}

/// A worktree's uncommitted files, as its card's footer counts them, with
/// what the count means in `title`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Uncommitted {
    pub text: String,
    pub title: String,
}

fn uncommitted(checkout: &CheckoutPage) -> Option<Uncommitted> {
    let git = checkout.git.as_ref().filter(|git| git.error.is_none())?;
    let files = match git.uncommitted? {
        0 => return None,
        n => crate::text::counted(n, "file", "files"),
    };
    Some(Uncommitted {
        title: format!("{files} changed, not committed"),
        text: files,
    })
}

/// What the waits of an item that keeps a worktree stand at, as its card
/// says them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeptCard {
    /// The item has no waits: it waits only for an agent.
    None,
    /// What it waits on that is not met.
    Open(Vec<ItemWait>),
    /// Every wait is met: each record that met one, and how it closed.
    Met(Vec<String>),
}

/// The state a worktree that an item keeps shows, with no agent in it: an
/// item with no waits waits only for an agent (Kept), one with an open wait
/// waits on it (KeptWaiting), and one whose waits are all met waits for the
/// coordinator to resume its agent or land its commits (KeptMet).
pub fn kept_state(card: &KeptCard) -> CardState {
    match card {
        KeptCard::None => CardState::Kept,
        KeptCard::Open(_) => CardState::KeptWaiting,
        KeptCard::Met(_) => CardState::KeptMet,
    }
}

/// The words of <checkout>, a worktree that <item> keeps with no agent in
/// it, given what its waits stand at.
pub fn kept_words(checkout: &CheckoutPage, item: RunningItem, card: &KeptCard) -> Words {
    let note = match card {
        KeptCard::None => None,
        KeptCard::Open(open) => Some(
            open.iter()
                .map(ItemWait::sentence)
                .collect::<Vec<_>>()
                .join(". "),
        ),
        KeptCard::Met(met) => Some(format!(
            "Every wait is met: {}. Land its commits, or resume its agent with builds item take.",
            met.join("; ")
        )),
    };
    Words {
        chip: kept_state(card).chip(),
        pill: None,
        items: vec![item],
        note,
        uncommitted: uncommitted(checkout),
        queued: None,
    }
}

/// The words of <checkout>, the person's main checkout, which has no state.
pub fn main_words(checkout: &CheckoutPage) -> Words {
    Words {
        chip: None,
        pill: None,
        items: Vec::new(),
        note: Some("Your checkout. No agent works or builds here.".into()),
        uncommitted: uncommitted(checkout),
        queued: None,
    }
}

/// The words of <checkout>, a live side worktree that is not an app checkout
/// and that no item keeps without an agent: <state> is its card's state
/// (`card_state`), <items> are its running items and <waits> is what the
/// worktree waits on, as `ranked` orders it.
pub fn words(
    checkout: &CheckoutPage,
    state: CardState,
    items: &[RunningItem],
    waits: &[ItemWait],
) -> Words {
    let wait = waits.first();
    let uncommitted = uncommitted(checkout);
    let claim_note = checkout
        .claim
        .as_ref()
        .and_then(|claim| claim.note_text().map(str::to_string))
        .filter(|note| !note.is_empty());
    let queued = match (state, wait) {
        (
            CardState::Queued(_),
            Some(ItemWait::AppBuild {
                queued: Some(queued),
                ..
            }),
        ) => Some(queued.clone()),
        _ => None,
    };
    // <said> is the sentence the card falls back on when no item runs, the
    // agent left no note and the card has no pill, whose title holds its own
    // sentence.
    let (pill, said) = match state {
        CardState::Queued(_) => (
            None,
            queued
                .as_ref()
                .map(|queued| queued.line.clone())
                .unwrap_or_default(),
        ),
        CardState::WaitingOnYou if wait.is_some_and(ItemWait::needs_person) => {
            (wait.and_then(ItemWait::pill), String::new())
        }
        CardState::ReadyToLand if wait == Some(&ItemWait::Unrecorded) => {
            (wait.and_then(ItemWait::pill), String::new())
        }
        CardState::Working if wait.is_some() => (
            wait.and_then(ItemWait::pill),
            wait.map(ItemWait::sentence).unwrap_or_default(),
        ),
        CardState::InReview(review) => (review.pill(), review.sentence()),
        // The card's errors say why.
        CardState::WaitsUnread => (None, String::new()),
        CardState::Free => (
            None,
            "No agent claims this worktree and every commit has landed.".to_string(),
        ),
        CardState::ReadyToLand if checkout.claim.is_none() => (
            None,
            "Commits wait to land and no agent claims this worktree.".to_string(),
        ),
        CardState::ReadyToLand => (None, "Waits for its agent to land it.".to_string()),
        CardState::WaitingOnYou => (None, "Waits on you.".to_string()),
        CardState::Paused => {
            let note = if waits.is_empty() {
                claim_note.clone().unwrap_or_else(|| {
                    "Its agent holds this worktree on purpose and runs no work.".to_string()
                })
            } else {
                waits
                    .iter()
                    .map(ItemWait::sentence)
                    .collect::<Vec<_>>()
                    .join(". ")
            };
            return Words {
                chip: state.chip(),
                pill: None,
                items: items.to_vec(),
                note: Some(note),
                uncommitted,
                queued: None,
            };
        }
        CardState::WindingDown => (None, "Finishing its last item.".to_string()),
        CardState::Working | CardState::Kept | CardState::KeptWaiting | CardState::KeptMet => {
            (None, "Working; its agent left no note.".to_string())
        }
    };
    // The note never repeats the pill's title.
    let note = match (items, &pill) {
        ([], None) => claim_note.or((!said.is_empty()).then_some(said)),
        ([], Some(pill)) => claim_note.filter(|note| *note != pill.title),
        _ => None,
    };
    Words {
        chip: state.chip(),
        pill,
        items: items.to_vec(),
        note,
        uncommitted,
        queued,
    }
}

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
