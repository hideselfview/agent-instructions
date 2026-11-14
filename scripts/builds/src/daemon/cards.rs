//! The words on a worktree's card: the chip in its top bar, the one plain
//! line of what is happening, and the state of its tree. The page draws
//! them; the times and the commits come from the checkout's own facts.

use super::model::CheckoutPage;

/// What a card says in words.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Words {
    /// The chip at the left of the top bar: only a worktree an agent holds
    /// or commits wait in has one.
    pub chip: Option<&'static str>,
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

/// The words of <checkout>, a live checkout that is not an app checkout.
pub fn words(checkout: &CheckoutPage) -> Words {
    let tree = tree(checkout);
    if checkout.main {
        return Words {
            chip: None,
            line: "Your checkout. No agent works or builds here.".into(),
            tree,
        };
    }
    let note = checkout
        .claim
        .as_ref()
        .and_then(|claim| claim.note.clone())
        .filter(|note| !note.is_empty());
    let (chip, line) = match checkout.state {
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
            note.unwrap_or_else(|| "Waits on you.".into()),
        ),
        Some("landing") => (
            Some("Working"),
            note.unwrap_or_else(|| "The coordinator lands commits from here.".into()),
        ),
        Some("held") => (
            Some("Working"),
            note.unwrap_or_else(|| "Held on purpose with no work running.".into()),
        ),
        Some("winding-down") => (
            Some("Working"),
            note.unwrap_or_else(|| "Finishing its last item.".into()),
        ),
        _ => (
            Some("Working"),
            note.unwrap_or_else(|| "Working; its agent left no note.".into()),
        ),
    };
    Words { chip, line, tree }
}

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
