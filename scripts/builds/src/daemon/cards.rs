//! The words on a worktree's card: the chip in its top bar, the one plain
//! line of what is happening, and the state of its tree. The page draws
//! them; the times and the commits come from the checkout's own facts.

use super::model::CheckoutPage;

/// What a running item waits on that is not met.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemWait {
    /// An open question for the person.
    Answer(String),
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
    Item { id: u32, title: String },
    /// Commits of a design branch that no recorded design change covers.
    Unrecorded,
    /// Commits waiting to land whose rules review `builds land` would not
    /// take yet (review_cards.rs).
    RulesReview(ReviewWait),
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
    /// This many confirmed findings wait on a commit that fixes them.
    Unfixed(usize),
    /// This many commits are neither ones the review read nor named fixes:
    /// they land as a change of their own.
    Unreviewed(usize),
}

impl ReviewWait {
    /// The card's chip.
    fn chip(self) -> &'static str {
        match self {
            ReviewWait::NotRun => "Needs review",
            ReviewWait::Running => "Reviewing",
            ReviewWait::ToSettle(_) => "Findings to settle",
            ReviewWait::Unfixed(_) => "Waiting on fixes",
            ReviewWait::Unreviewed(_) => "Unreviewed commits",
        }
    }

    fn line(self) -> String {
        let findings = |n: usize, what: &str| match n {
            1 => format!("1 {what} finding waits"),
            n => format!("{n} {what} findings wait"),
        };
        match self {
            ReviewWait::NotRun => {
                "Waits on the coordinator to review its commits against the rules.".to_string()
            }
            ReviewWait::Running => "The rules review of its commits runs.".to_string(),
            ReviewWait::ToSettle(n) => format!(
                "{} on the coordinator to settle {}.",
                findings(n, "review"),
                if n == 1 { "it" } else { "them" }
            ),
            ReviewWait::Unfixed(n) => format!(
                "{} on a commit that fixes {}.",
                findings(n, "confirmed review"),
                if n == 1 { "it" } else { "them" }
            ),
            ReviewWait::Unreviewed(1) => "The review did not read 1 commit, and no finding names it as a fix: it lands as a change of its own.".to_string(),
            ReviewWait::Unreviewed(n) => format!(
                "The review did not read {n} commits, and no finding names them as fixes: they land as a change of their own."
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
    /// "Asked 8m ago to run macos macos-tests."
    pub line: String,
    /// The steps in `line`, which the page draws in the mono face.
    pub steps: Option<String>,
}

impl ItemWait {
    /// What the person has to do comes before another item.
    pub fn rank(&self) -> u8 {
        match self {
            ItemWait::Answer(_) => 0,
            ItemWait::Review { .. } => 1,
            ItemWait::AppBuild { .. } => 2,
            ItemWait::Item { .. } => 3,
            ItemWait::Unrecorded => 4,
            ItemWait::RulesReview(_) => 5,
        }
    }

    /// Does only the person's answer or review end this wait?
    pub fn needs_person(&self) -> bool {
        matches!(self, ItemWait::Answer(_) | ItemWait::Review { .. })
    }

    /// The words of the wait. <titled> says an item's title is already the
    /// card's main line, so a design change's title belongs in the words;
    /// without one, the change's title is the main line.
    fn line(&self, titled: bool) -> String {
        match self {
            ItemWait::Answer(title) => format!("Waits on your answer: {title}"),
            ItemWait::Review { id, title } if titled => {
                format!("Waits on your review of design change {id}: {title}")
            }
            ItemWait::Review { id, .. } => {
                format!("Waits on your review of design change {id}")
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
            ItemWait::Item { id, title } => format!("Waits on #{id}: {title}"),
            ItemWait::Unrecorded => {
                "Waits on the design session to record its design change.".to_string()
            }
            ItemWait::RulesReview(review) => review.line(),
        }
    }
}

/// Does a card in <state> stay Working instead of Ready to land? Ready to
/// land means commits past origin/main and nothing else going on: no agent
/// shell active in the worktree (<shell>), no app build of its branch and no
/// running item that waits on another (<wait>).
pub fn keeps_working(state: Option<&str>, shell: bool, wait: Option<&ItemWait>) -> bool {
    state == Some("ready-to-land")
        && (shell
            || matches!(
                wait,
                Some(ItemWait::AppBuild { .. } | ItemWait::Item { .. })
            ))
}

/// Is a worktree in <state> in review instead of Ready to land? Its commits
/// wait to land and the rules review <wait> stands in the way.
pub fn in_review(state: Option<&str>, wait: Option<&ItemWait>) -> bool {
    state == Some("ready-to-land") && matches!(wait, Some(ItemWait::RulesReview(_)))
}

/// Does a worktree in <state> wait for an app build that has not started? Its
/// running item's build waits in a lane's queue, so the card says so in place
/// of Working.
pub fn waits_in_queue(state: Option<&str>, wait: Option<&ItemWait>) -> bool {
    matches!(state, Some("working"))
        && matches!(wait, Some(ItemWait::AppBuild { running: false, .. }))
}

/// What a card says in words.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Words {
    /// The chip at the left of the top bar: only a worktree an agent holds
    /// or commits wait in has one.
    pub chip: Option<&'static str>,
    /// The titles of the items running here, the card's main lines: one per
    /// item. They replace the claim note. A design branch with no running
    /// item shows its design change's title.
    pub titles: Vec<String>,
    /// The plain line of what the worktree waits on or does. With a title
    /// above it, the claim note is left out.
    pub line: String,
    /// `clean` or `N files changed`; None when git was not read here.
    pub tree: Option<String>,
    /// The place in a lane's queue of a worktree that waits for its app
    /// build to start.
    pub queued: Option<QueuedBuild>,
}

fn tree(checkout: &CheckoutPage) -> Option<String> {
    let git = checkout.git.as_ref().filter(|git| git.error.is_none())?;
    Some(match git.uncommitted? {
        0 => "clean".to_string(),
        1 => "1 file changed".to_string(),
        n => format!("{n} files changed"),
    })
}

/// The words of <checkout>, a live checkout that is not an app checkout:
/// <titles> are its running items' and <wait> is what they wait on.
pub fn words(checkout: &CheckoutPage, titles: &[String], wait: Option<&ItemWait>) -> Words {
    let tree = tree(checkout);
    if checkout.main {
        return Words {
            chip: None,
            titles: Vec::new(),
            line: "Your checkout. No agent works or builds here.".into(),
            tree,
            queued: None,
        };
    }
    // A running item's title replaces the claim note.
    let note = checkout
        .claim
        .as_ref()
        .and_then(|claim| claim.note.clone())
        .filter(|note| !note.is_empty());
    let titled = !titles.is_empty();
    let note = if titled { None } else { note };
    let fallback = |text: &str| {
        if titled {
            String::new()
        } else {
            text.to_string()
        }
    };
    let (chip, line) = match checkout.state {
        Some("queued") => match wait {
            Some(ItemWait::AppBuild { lane, .. }) => (Some(queued_chip(lane)), String::new()),
            _ => (Some("Working"), String::new()),
        },
        Some("waiting-on-you") if wait.is_some_and(ItemWait::needs_person) => (
            Some("Waiting on you"),
            wait.map(|w| w.line(titled)).unwrap_or_default(),
        ),
        Some("ready-to-land") if wait == Some(&ItemWait::Unrecorded) => (
            Some("Ready to land"),
            wait.map(|w| w.line(titled)).unwrap_or_default(),
        ),
        Some("working") if wait.is_some() => (
            Some("Working"),
            wait.map(|w| w.line(titled)).unwrap_or_default(),
        ),
        Some("in-review") => match wait {
            Some(ItemWait::RulesReview(review)) => (Some(review.chip()), review.line()),
            _ => (Some("Needs review"), String::new()),
        },
        Some("free") => (None, "Free: no claim, every commit landed.".to_string()),
        Some("ready-to-land") if checkout.claim.is_none() => (
            Some("Ready to land"),
            "Commits wait to land and no agent claims this worktree.".to_string(),
        ),
        Some("ready-to-land") => (
            Some("Ready to land"),
            "Waits to be landed by the coordinator.".to_string(),
        ),
        Some("waiting-on-you") => (
            Some("Waiting on you"),
            note.unwrap_or_else(|| fallback("Waits on you.")),
        ),
        Some("landing") => (
            Some("Working"),
            note.unwrap_or_else(|| fallback("The coordinator lands commits from here.")),
        ),
        Some("held") => (
            Some("Working"),
            note.unwrap_or_else(|| fallback("Held on purpose with no work running.")),
        ),
        Some("winding-down") => (
            Some("Working"),
            note.unwrap_or_else(|| fallback("Finishing its last item.")),
        ),
        _ => (
            Some("Working"),
            note.unwrap_or_else(|| fallback("Working; its agent left no note.")),
        ),
    };
    Words {
        chip,
        titles: match wait {
            Some(ItemWait::Review { title, .. }) if !titled => vec![title.clone()],
            _ => titles.to_vec(),
        },
        line: match (checkout.state, wait) {
            (
                Some("queued"),
                Some(ItemWait::AppBuild {
                    queued: Some(queued),
                    ..
                }),
            ) => queued.line.clone(),
            _ => line,
        },
        tree,
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
