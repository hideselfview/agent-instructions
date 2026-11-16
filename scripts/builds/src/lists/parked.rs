//! Where an item holds a worktree with no agent: `builds item park` leaves an
//! item's work in its worktree on its branch while the item waits in the
//! queue, and taking the item moves it to running there (take.rs). A parked
//! worktree has no claim, since no agent works there, and `builds new`,
//! `prune` and `retire` never hand it out or remove it. `findings` is what
//! `builds health` says about parked items.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::health::Advice;
use crate::lists::item::{self, Item};
use crate::lists::running::side_worktree_split;
use crate::lists::store::Loaded;
use crate::lists::{Id, Lists, Stamp};

/// A worktree an open item holds parked, in one repo (an item that spans
/// repos has one of these per repo): its work waits there on the branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parked {
    /// The absolute path of a side worktree of a project repo.
    pub worktree: String,
    pub branch: String,
    pub since: Stamp,
}

/// Parks open item <id> in <places>, all at the moment the first of them
/// says.
pub fn park(db: &Connection, id: Id, places: &[Parked]) -> Result<()> {
    let Some(first) = places.first() else {
        fail!("an item parks in at least one worktree")
    };
    item::require_open_item(db, id)?;
    db.execute(
        "INSERT INTO parked (item_id, since_at, since_by) VALUES (?1, ?2, ?3)",
        params![id, first.since.at, first.since.by],
    )
    .sql()?;
    for place in places {
        let Some((parent, repo, checkout)) = side_worktree_split(&place.worktree) else {
            fail!(
                "{} is not a side worktree (<parent>/<repo>-worktrees/side-N)",
                place.worktree
            );
        };
        db.execute(
            "INSERT INTO parked_place (item_id, repo, parent, checkout, branch) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, repo, parent, checkout, place.branch],
        )
        .sql()?;
    }
    Ok(())
}

/// Ends item <id>'s parking wherever it is parked, if it is.
pub fn end(db: &Connection, id: Id) -> Result<()> {
    db.execute("DELETE FROM parked WHERE item_id = ?1", [id])
        .sql()?;
    Ok(())
}

/// Where every parked item is parked, by item.
pub fn read(db: &Connection) -> Result<BTreeMap<Id, Vec<Parked>>> {
    let mut statement = db
        .prepare(
            "SELECT k.item_id, p.worktree, p.branch, k.since_at, k.since_by \
             FROM parked k JOIN parked_place p ON p.item_id = k.item_id ORDER BY k.item_id, p.repo",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                Parked {
                    worktree: row.get(1)?,
                    branch: row.get(2)?,
                    since: Stamp {
                        at: row.get(3)?,
                        by: row.get(4)?,
                    },
                },
            ))
        })
        .sql()?;
    let mut all: BTreeMap<Id, Vec<Parked>> = BTreeMap::new();
    for row in rows {
        let (id, parked) = row.sql()?;
        all.entry(id).or_default().push(parked);
    }
    Ok(all)
}

/// The open item parked in <worktree>, if one is: a worktree holds one parked
/// item alone (check.rs).
pub fn parked_in<'a>(lists: &'a Lists, worktree: &str) -> Option<&'a Item> {
    lists
        .items
        .iter()
        .find(|item| item.parked.iter().any(|p| p.worktree == worktree))
}

/// The worktrees an open item of any store is parked in; Err names a store
/// that does not load, whose parked items are unknown.
pub fn parked_worktrees(
    loaded: &std::result::Result<Vec<Loaded>, String>,
) -> std::result::Result<Vec<String>, String> {
    let mut parked = Vec::new();
    for project in loaded.as_ref().map_err(String::clone)? {
        let lists = project
            .lists
            .as_ref()
            .map_err(|fault| format!("project {}'s lists: {fault}", project.project))?;
        for item in &lists.items {
            parked.extend(item.parked.iter().map(|p| p.worktree.clone()));
        }
    }
    Ok(parked)
}

/// The worktrees an open item is parked in, read from every store; fails
/// when a store does not load, since `builds new`, `prune` and `retire` must
/// never hand out or remove a parked worktree.
pub fn parked_worktrees_now(places: &crate::places::Places) -> Result<Vec<String>> {
    parked_worktrees(&crate::lists::store::load_all(places)).map_err(|fault| {
        crate::fail::Fail::Message(format!(
            "builds cannot tell which worktrees items are parked in: {fault}"
        ))
    })
}

/// What `builds health` says about parked items: one line for each parked
/// item whose worktree is not a live worktree, since its work is no longer
/// where the item says it waits.
pub fn findings(loaded: &std::result::Result<Vec<Loaded>, String>) -> Result<Vec<Advice>> {
    let Ok(loaded) = loaded else {
        return Ok(Vec::new());
    };
    let mut findings = Vec::new();
    for project in loaded {
        let Ok(lists) = &project.lists else { continue };
        for item in &lists.items {
            for parked in &item.parked {
                if crate::lists::running::live_worktree(&parked.worktree)?.is_some() {
                    continue;
                }
                findings.push(Advice::run(
                    format!(
                        "lists: item {} \"{}\" is parked in {}, which is not a live worktree; see what it holds with",
                        item.id.0, item.title, parked.worktree
                    ),
                    format!("builds item show {}", item.id.0),
                ));
            }
        }
    }
    Ok(findings)
}
