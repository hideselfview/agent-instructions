//! Shots of scenes whose board change waits on app work that is still open.
//!
//! A design branch redraws a board ahead of the app, and records in its design
//! change which open item does the app work of each board place
//! (`builds help lists`, design record, `places`). The shot of that scene
//! differs from its new board until the item lands, so an app build of the
//! branch leaves it out, and land counts it covered by the design record.
//!
//! A scene is waiting when its board file is a place of an open design change
//! of the repo, the branch changes that file, and the place's app work is an
//! open item. A shot's name is its board's name (`mac.welcome.open-failed` is
//! design/mocks/mac.welcome.open-failed.dc.html).

use std::collections::{BTreeMap, HashSet};

use rusqlite::Connection;

use crate::app_build::Pick;
use crate::app_config::PickKind;
use crate::fail::Result;
use crate::lane::Lane;
use crate::lists::design_paths::board_scene;
use crate::lists::{Id, Lists, Resolution};
use crate::shots_declared::ShotDeclarations;
use crate::text::note;

/// A scene whose board change waits on app work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    /// The scene's shot name, the board's name.
    pub scene: String,
    /// The open items that do the app work, in id order.
    pub items: Vec<Id>,
}

impl Waiting {
    /// The scene and the items it waits on, as a line of output says them.
    pub fn said(&self) -> String {
        let items: Vec<String> = self.items.iter().map(|id| id.0.to_string()).collect();
        format!("{} waits on item {}", self.scene, items.join(", "))
    }
}

/// The scenes of <repo>'s boards among <changed> whose place in an open design
/// change has an open item as its app work, by scene name.
pub fn waiting_in(lists: &Lists, repo: &str, changed: &[String]) -> Vec<Waiting> {
    let mut found: BTreeMap<String, Vec<Id>> = BTreeMap::new();
    for design in lists.designs.iter().filter(|design| design.repo == repo) {
        for (path, resolution) in &design.places {
            let (Some(Resolution::Item(item)), Some(scene)) = (resolution, board_scene(path))
            else {
                continue;
            };
            if changed.contains(path) && lists.item(*item).is_some() {
                let items = found.entry(scene.to_string()).or_default();
                if !items.contains(item) {
                    items.push(*item);
                    items.sort();
                }
            }
        }
    }
    found
        .into_iter()
        .map(|(scene, items)| Waiting { scene, items })
        .collect()
}

/// The scenes of <repo> among the boards <changed> that the project's store,
/// open as <conn>, says wait on open app work.
pub fn waiting(conn: &Connection, repo: &str, changed: &[String]) -> Result<Vec<Waiting>> {
    Ok(waiting_in(&crate::lists::store::read(conn)?, repo, changed))
}

/// The names of <waiting>, for a check of which shots a prefix selects.
pub fn scenes(waiting: &[Waiting]) -> HashSet<String> {
    waiting.iter().map(|w| w.scene.clone()).collect()
}

/// What a shots prefix becomes when some scenes it selects are waiting: the
/// names it still runs and the scenes it leaves out.
#[derive(Debug, PartialEq, Eq)]
pub struct Split<'a> {
    pub kept: Vec<String>,
    pub left_out: Vec<&'a Waiting>,
}

/// <prefix> split over the shot names <declared>, or None when it selects no
/// waiting scene. The prefix selects each name that starts with it, so each
/// kept name is run as a prefix of its own.
pub fn split<'a>(
    prefix: &str,
    declared: &HashSet<String>,
    waiting: &'a [Waiting],
) -> Option<Split<'a>> {
    let left_out: Vec<&Waiting> = waiting
        .iter()
        .filter(|w| declared.contains(&w.scene) && w.scene.starts_with(prefix))
        .collect();
    if left_out.is_empty() {
        return None;
    }
    let mut kept: Vec<String> = declared
        .iter()
        .filter(|name| name.starts_with(prefix) && !left_out.iter().any(|w| w.scene == **name))
        .cloned()
        .collect();
    kept.sort();
    Some(Split { kept, left_out })
}

/// <picks> without the shots of waiting scenes: each shots pick whose prefix
/// selects a waiting scene is replaced by one pick for each name it still
/// runs. A run of a whole lane (`--all-shots`) is kept as it is. Says each
/// scene left out and the items it waits on.
pub fn leave_out(
    picks: Vec<Pick>,
    waiting: &[Waiting],
    declarations: &mut ShotDeclarations<'_>,
    commit: &str,
) -> Result<Vec<Pick>> {
    if waiting.is_empty() {
        return Ok(picks);
    }
    let mut out = Vec::new();
    for pick in picks {
        let Pick::Shots(lane, prefix) = &pick else {
            out.push(pick);
            continue;
        };
        if pick.is_all_shots() {
            out.push(pick);
            continue;
        }
        let Some(names) = declarations.names(*lane, commit) else {
            out.push(pick);
            continue;
        };
        let Some(Split { kept, left_out }) = split(prefix.as_str(), &names, waiting) else {
            out.push(pick);
            continue;
        };
        for scene in &left_out {
            note(format!(
                "the {} shot {} is left out: its board change waits on app work ({}); land counts it covered by the design record",
                lane.word(),
                scene.scene,
                scene.said()
            ));
        }
        for name in kept {
            if let Some(longer) = left_out.iter().find(|w| w.scene.starts_with(&name)) {
                note(format!(
                    "the {} shot {name} is also the start of the left out shot {}, so its run takes that shot too",
                    lane.word(),
                    longer.scene
                ));
            }
            out.push(Pick::from_flag(*lane, PickKind::Shots, &name)?);
        }
    }
    Ok(out)
}

/// The lines land prints for the scenes of <waiting> that the landing's
/// build left out.
pub fn landing_lines(waiting: &[Waiting], lane: Lane) -> Vec<String> {
    waiting
        .iter()
        .filter(|w| w.scene.starts_with(lane.word()))
        .map(|w| {
            format!(
                "{WAITING} the {} shot {}: its board change waits on app work, so no app build needs to run it; the design record covers it",
                lane.word(),
                w.said()
            )
        })
        .collect()
}

/// What starts each line that tells of a scene that waits on app work.
pub const WAITING: &str = "shots waiting on app work:";

#[cfg(test)]
#[path = "app_waiting_shots_tests.rs"]
mod tests;
