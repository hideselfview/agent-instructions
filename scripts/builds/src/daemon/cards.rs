//! The words on a worktree's card: the chip in its top bar, the pill that
//! names what the card waits on, the running items it lists, its note (when
//! no item runs, or what a held worktree waits on), and its uncommitted
//! files. The page draws them; the times and the commits come from the
//! checkout's own facts.

use super::model::CheckoutPage;

/// What a running item waits on that is not met.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemWait {
    /// An open question for the person.
    Answer { id: u32, title: String },
    /// A design change that waits on the person's review.
    Review { id: u32, title: String },
    /// An app build of the branch runs or waits in a lane: `lane` is `Mac`
    /// or `Android`, `running` says whether it builds now and `how` says how
    /// far it is.
    AppBuild {
        lane: String,
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
    /// A condition in words, which holds until someone removes it from the
    /// item.
    Condition(String),
}

/// Where the rules review of a branch stands while `builds land` would refuse
/// its commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewWait {
    /// No review read these commits, or a group still has to answer over
    /// them: the coordinator runs the review.
    NotRun,
    /// A review of the branch's newest commit runs now.
    Running,
    /// This many findings wait for the coordinator to settle them.
    ToSettle(usize),
    /// This many confirmed findings are open: they wait for a review that
    /// closes them.
    Open(usize),
    /// This many files changed since the last review: `builds review` reads
    /// those changes in a new run.
    Unreviewed(usize),
}

impl ReviewWait {
    /// The card's chip.
    fn chip(self) -> &'static str {
        match self {
            ReviewWait::NotRun => "Needs review",
            ReviewWait::Running => "Reviewing",
            ReviewWait::ToSettle(_) => "Findings to settle",
            ReviewWait::Open(_) => "Open findings",
            ReviewWait::Unreviewed(_) => "Unreviewed changes",
        }
    }

    /// The pill in the card's bar; None when the chip already names the wait.
    fn pill(self) -> Option<Pill> {
        let text = match self {
            ReviewWait::NotRun | ReviewWait::Running => return None,
            ReviewWait::ToSettle(n) | ReviewWait::Open(n) => {
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
            ReviewWait::ToSettle(n) => format!(
                "{} on the coordinator to settle {}.",
                findings(n, "review"),
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
            ItemWait::Condition(_) => 6,
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
            } => format!("{lane} build"),
            ItemWait::AppBuild { running: false, .. } => return None,
            ItemWait::Item { id, .. } | ItemWait::Landing { id, .. } => format!("After #{id}"),
            ItemWait::Unrecorded => "No design change".to_string(),
            ItemWait::RulesReview(review) => return review.pill(),
            ItemWait::Condition(_) => return None,
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
            } => format!("{lane} app build: {how}"),
            ItemWait::AppBuild { lane, how, .. } => {
                format!("Waits on the {lane} app build: {how}")
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
            ItemWait::Condition(text) => format!("Waits until {text}"),
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

/// The state a worktree's card shows. <state> is what its claim and commits
/// say (model.rs worktree_state), <shell> says whether an agent shell is
/// active in it, and <waits> is what it waits on as `ranked` orders it, or
/// None when the daemon could not read what its running items wait on: then
/// no wait can set the state, and the state is waits-unread. A main
/// checkout has no state.
pub fn card_state(
    state: Option<&'static str>,
    shell: bool,
    waits: Option<&[ItemWait]>,
) -> Option<&'static str> {
    state?;
    let Some(waits) = waits else {
        return Some("waits-unread");
    };
    let wait = waits.first();
    if wait.is_some_and(ItemWait::needs_person) {
        return Some("waiting-on-you");
    }
    let mut state = state;
    if keeps_working(state, shell, wait) {
        state = Some("working");
    }
    if in_review(state, wait) {
        state = Some("in-review");
    }
    if waits_in_queue(state, wait) {
        state = Some("queued");
    }
    state
}

/// Does a card in <state> stay Working instead of Ready to land? Ready to
/// land means commits past origin/main and nothing else going on: no agent
/// shell active in the worktree (<shell>), no app build of its branch and no
/// running item that waits on another item or on a design change to land
/// (<wait>).
fn keeps_working(state: Option<&str>, shell: bool, wait: Option<&ItemWait>) -> bool {
    state == Some("ready-to-land")
        && (shell
            || matches!(
                wait,
                Some(ItemWait::AppBuild { .. } | ItemWait::Item { .. } | ItemWait::Landing { .. })
            ))
}

/// Is a worktree in <state> in review instead of Ready to land? Its commits
/// wait to land and the rules review <wait> stands in the way.
fn in_review(state: Option<&str>, wait: Option<&ItemWait>) -> bool {
    state == Some("ready-to-land") && matches!(wait, Some(ItemWait::RulesReview(_)))
}

/// Does a worktree in <state> wait for an app build that has not started? Its
/// running item's build waits in a lane's queue, so the card says so in place
/// of Working.
fn waits_in_queue(state: Option<&str>, wait: Option<&ItemWait>) -> bool {
    matches!(state, Some("working"))
        && matches!(wait, Some(ItemWait::AppBuild { running: false, .. }))
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
/// item with no waits waits only for an agent (`kept`), one with an open wait
/// waits on it (`kept-waiting`), and one whose waits are all met waits for
/// the coordinator to resume its agent or land its commits (`kept-met`).
pub fn kept_state(card: &KeptCard) -> &'static str {
    match card {
        KeptCard::None => "kept",
        KeptCard::Open(_) => "kept-waiting",
        KeptCard::Met(_) => "kept-met",
    }
}

/// The words of <checkout>, a worktree that <item> keeps with no agent in
/// it, given what its waits stand at.
pub fn kept_words(checkout: &CheckoutPage, item: RunningItem, card: &KeptCard) -> Words {
    let (chip, note) = match card {
        KeptCard::None => ("Ready", None),
        KeptCard::Open(open) => (
            "Waiting",
            Some(
                open.iter()
                    .map(ItemWait::sentence)
                    .collect::<Vec<_>>()
                    .join(". "),
            ),
        ),
        KeptCard::Met(met) => (
            "Waits met",
            Some(format!(
                "Every wait is met: {}. Land its commits, or resume its agent with builds item take.",
                met.join("; ")
            )),
        ),
    };
    Words {
        chip: Some(chip),
        pill: None,
        items: vec![item],
        note,
        uncommitted: uncommitted(checkout),
        queued: None,
    }
}

/// The words of <checkout>, a live checkout that is not an app checkout:
/// <items> are its running items and <waits> is what the worktree waits on,
/// as `ranked` orders it: the daemon set the card's state from the first.
pub fn words(checkout: &CheckoutPage, items: &[RunningItem], waits: &[ItemWait]) -> Words {
    let wait = waits.first();
    let uncommitted = uncommitted(checkout);
    if checkout.main {
        return Words {
            chip: None,
            pill: None,
            items: Vec::new(),
            note: Some("Your checkout. No agent works or builds here.".into()),
            uncommitted,
            queued: None,
        };
    }
    let claim_note = checkout
        .claim
        .as_ref()
        .and_then(|claim| claim.note.clone())
        .filter(|note| !note.is_empty());
    // <said> is the sentence the card falls back on when no item runs, the
    // agent left no note and the card has no pill, whose title holds its own
    // sentence.
    let (chip, pill, said) = match checkout.state {
        Some("queued") => match wait {
            Some(ItemWait::AppBuild {
                lane,
                queued: Some(queued),
                ..
            }) => (Some(queued_chip(lane)), None, queued.line.clone()),
            Some(ItemWait::AppBuild { lane, .. }) => (Some(queued_chip(lane)), None, String::new()),
            _ => (Some("Working"), None, String::new()),
        },
        Some("waiting-on-you") if wait.is_some_and(ItemWait::needs_person) => (
            Some("Waiting on you"),
            wait.and_then(ItemWait::pill),
            String::new(),
        ),
        Some("ready-to-land") if wait == Some(&ItemWait::Unrecorded) => (
            Some("Ready to land"),
            wait.and_then(ItemWait::pill),
            String::new(),
        ),
        Some("working") if wait.is_some() => (
            Some("Working"),
            wait.and_then(ItemWait::pill),
            wait.map(ItemWait::sentence).unwrap_or_default(),
        ),
        Some("in-review") => match wait {
            Some(ItemWait::RulesReview(review)) => {
                (Some(review.chip()), review.pill(), review.sentence())
            }
            _ => (Some("Needs review"), None, String::new()),
        },
        // The card's errors say why.
        Some("waits-unread") => (
            Some("Can't read what this worktree waits on"),
            None,
            String::new(),
        ),
        Some("free") => (
            None,
            None,
            "No agent claims this worktree and every commit has landed.".to_string(),
        ),
        Some("ready-to-land") if checkout.claim.is_none() => (
            Some("Ready to land"),
            None,
            "Commits wait to land and no agent claims this worktree.".to_string(),
        ),
        Some("ready-to-land") => (
            Some("Ready to land"),
            None,
            "Waits for the coordinator to land it.".to_string(),
        ),
        Some("waiting-on-you") => (Some("Waiting on you"), None, "Waits on you.".to_string()),
        Some("held") => {
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
                chip: Some("Paused"),
                pill: None,
                items: items.to_vec(),
                note: Some(note),
                uncommitted,
                queued: None,
            };
        }
        Some("winding-down") => (
            Some("Working"),
            None,
            "Finishing its last item.".to_string(),
        ),
        _ => (
            Some("Working"),
            None,
            "Working; its agent left no note.".to_string(),
        ),
    };
    // The note never repeats the pill's title.
    let note = match (items, &pill) {
        ([], None) => claim_note.or((!said.is_empty()).then_some(said)),
        ([], Some(pill)) => claim_note.filter(|note| *note != pill.title),
        _ => None,
    };
    Words {
        chip,
        pill,
        items: items.to_vec(),
        note,
        uncommitted,
        queued: match (checkout.state, wait) {
            (
                Some("queued"),
                Some(ItemWait::AppBuild {
                    queued: Some(queued),
                    ..
                }),
            ) => Some(queued.clone()),
            _ => None,
        },
    }
}

/// The chip of a worktree that waits for the app build lane <lane> to start
/// its build.
fn queued_chip(lane: &str) -> &'static str {
    if lane == "Android" {
        "Waiting on Android"
    } else {
        "Waiting on Mac"
    }
}

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
