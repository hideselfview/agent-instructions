//! The daemon's thread that records each change of a worktree card's
//! dashboard state (card_state.sql). After each change of the daemon's
//! picture, and at most once every `EVERY`, it judges every card with the
//! code that draws it (`Model::cards`) and writes each project's changes to
//! that project's store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use super::model::{CardLists, CardNow, Model};
use crate::card_state::{self, Seen};
use crate::fail::{Context, Result};

/// The shortest time between two recordings.
pub const EVERY: Duration = Duration::from_secs(5);

/// The cards of <cards> that show under each project, and the worktrees whose
/// project the daemon could not choose.
pub fn by_project(cards: &[CardNow]) -> (BTreeMap<String, Vec<Seen>>, BTreeSet<String>) {
    let mut shown: BTreeMap<String, Vec<Seen>> = BTreeMap::new();
    let mut unread = BTreeSet::new();
    for card in cards {
        match &card.lists {
            CardLists::Project(project) => shown.entry(project.clone()).or_default().push(Seen {
                worktree: card.worktree.clone(),
                state: card.state.name(),
                items: card.items.iter().copied().collect(),
            }),
            CardLists::None => {}
            CardLists::Unread => {
                unread.insert(card.worktree.clone());
            }
        }
    }
    (shown, unread)
}

/// Judges every card now and writes, as of <at>, each project's changes to
/// its store. Records nothing before the daemon's first reading of the
/// processes, which says where agent shells are active and which app builds
/// run. A store that refuses the write is said and the others still get
/// theirs.
pub fn record_once(model: &Model, at: i64) -> Result<()> {
    let Some(cards) = model.cards()? else {
        return Ok(());
    };
    let (shown, unread) = by_project(&cards);
    for store in crate::lists::store::stores(&model.places)? {
        let cards = shown.get(&store.project).map(Vec::as_slice).unwrap_or(&[]);
        let written = crate::db::open(&store.database(), crate::db::Make::Never)
            .and_then(|mut db| card_state::follow(&mut db, at, cards, &unread));
        if let Err(error) = written {
            crate::output::note(format!(
                "recording the cards' states in project {}: {}",
                store.project,
                error.text()
            ));
        }
    }
    Ok(())
}

/// Starts the thread that records the cards' states after each change of
/// <model>'s picture.
pub fn start(model: Arc<Model>) -> Result<()> {
    std::thread::Builder::new()
        .name("builds-card-states".into())
        .spawn(move || {
            let mut seen = 0;
            loop {
                seen = model.wait_change(seen);
                if let Err(error) = record_once(&model, super::model::now_seconds()) {
                    crate::output::note(format!("recording the cards' states: {}", error.text()));
                }
                std::thread::sleep(EVERY);
            }
        })
        .context(|| "starting a thread".to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "card_recorder_tests.rs"]
mod tests;
