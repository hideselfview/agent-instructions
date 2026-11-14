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
    /// Another item.
    Item { id: u32, title: String },
    /// Commits of a design branch that no recorded design change covers.
    Unrecorded,
}

impl ItemWait {
    /// What the person has to do comes before another item.
    pub fn rank(&self) -> u8 {
        match self {
            ItemWait::Answer(_) => 0,
            ItemWait::Review { .. } => 1,
            ItemWait::Item { .. } => 2,
            ItemWait::Unrecorded => 3,
        }
    }

    /// Does only the person's answer or review end this wait?
    pub fn needs_person(&self) -> bool {
        matches!(self, ItemWait::Answer(_) | ItemWait::Review { .. })
    }

    fn line(&self) -> String {
        match self {
            ItemWait::Answer(title) => format!("Waits on your answer: {title}"),
            ItemWait::Review { id, title } => {
                format!("Waits on your review of design change {id}: {title}")
            }
            ItemWait::Item { id, title } => format!("Waits on #{id}: {title}"),
            ItemWait::Unrecorded => {
                "Waits on the design session to record its design change.".to_string()
            }
        }
    }
}

/// What a card says in words.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Words {
    /// The chip at the left of the top bar: only a worktree an agent holds
    /// or commits wait in has one.
    pub chip: Option<&'static str>,
    /// The titles of the items running here, the card's main lines: one per
    /// item. They replace the claim note.
    pub titles: Vec<String>,
    /// The plain line of what the worktree waits on or does. With a title
    /// above it, the claim note is left out.
    pub line: String,
    /// `clean` or `N files changed`; None when git was not read here.
    pub tree: Option<String>,
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
        Some("waiting-on-you") if wait.is_some_and(ItemWait::needs_person) => (
            Some("Waiting on you"),
            wait.map(ItemWait::line).unwrap_or_default(),
        ),
        Some("ready-to-land") if wait == Some(&ItemWait::Unrecorded) => (
            Some("Ready to land"),
            wait.map(ItemWait::line).unwrap_or_default(),
        ),
        Some("working") if wait.is_some() => (
            Some("Working"),
            wait.map(ItemWait::line).unwrap_or_default(),
        ),
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
        titles: titles.to_vec(),
        line,
        tree,
    }
}

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
