//! Where an item keeps a worktree with no agent: `builds item stop` records
//! the worktree and branch of an item whose run ended with work left in
//! them, and the item waits in the queue, ready, until an agent takes it
//! there (take.rs). A kept worktree has no claim, since no agent works
//! there, and `builds new`, `prune` and `retire` never hand it out or remove
//! it. `findings` is what `builds health` says about kept work.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::health::Advice;
use crate::lists::item::{self, Item};
use crate::lists::running::{PlaceTable, insert_places};
use crate::lists::store::Loaded;
use crate::lists::{Id, Lists, Stamp};

/// A worktree an open item keeps, in one repo (an item that spans repos has
/// one of these per repo): its work waits there on the branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptWork {
    /// The absolute path of a side worktree of a project repo.
    pub worktree: String,
    pub branch: String,
    pub since: Stamp,
}

/// Has open item <id> keep <places>, stamped with the `since` of the first
/// place.
pub fn keep(db: &Connection, id: Id, places: &[KeptWork]) -> Result<()> {
    let Some(first) = places.first() else {
        fail!("an item keeps at least one worktree")
    };
    item::require_open_item(db, id)?;
    db.execute(
        "INSERT INTO kept_work (item_id, since_at, since_by) VALUES (?1, ?2, ?3)",
        params![id, first.since.at, first.since.by],
    )
    .sql()?;
    insert_places(
        db,
        PlaceTable::Kept,
        id,
        places
            .iter()
            .map(|place| (place.worktree.as_str(), place.branch.as_str())),
    )
}

/// Ends what item <id> keeps, if it keeps anything.
pub fn end(db: &Connection, id: Id) -> Result<()> {
    db.execute("DELETE FROM kept_work WHERE item_id = ?1", [id])
        .sql()?;
    Ok(())
}

/// What every item keeps, by item.
pub fn read(db: &Connection) -> Result<BTreeMap<Id, Vec<KeptWork>>> {
    let mut statement = db
        .prepare(
            "SELECT k.item_id, p.worktree, p.branch, k.since_at, k.since_by \
             FROM kept_work k JOIN kept_work_place p ON p.item_id = k.item_id ORDER BY k.item_id, p.repo",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                KeptWork {
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
    let mut all: BTreeMap<Id, Vec<KeptWork>> = BTreeMap::new();
    for row in rows {
        let (id, kept) = row.sql()?;
        all.entry(id).or_default().push(kept);
    }
    Ok(all)
}

/// The open item that keeps <worktree>, if one does: a worktree holds one
/// item's kept work alone (check.rs).
pub fn kept_in<'a>(lists: &'a Lists, worktree: &str) -> Option<&'a Item> {
    lists
        .items
        .iter()
        .find(|item| item.kept.iter().any(|p| p.worktree == worktree))
}

/// The worktrees an open item of any store keeps; Err names a store that
/// does not load, whose kept work is unknown.
pub fn kept_worktrees(
    loaded: &std::result::Result<Vec<Loaded>, String>,
) -> std::result::Result<Vec<String>, String> {
    let mut kept = Vec::new();
    for project in loaded.as_ref().map_err(String::clone)? {
        let lists = project
            .lists
            .as_ref()
            .map_err(|fault| format!("project {}'s lists: {fault}", project.project))?;
        for item in &lists.items {
            kept.extend(item.kept.iter().map(|p| p.worktree.clone()));
        }
    }
    Ok(kept)
}

/// What `builds health` says about kept work: one line for each item that
/// keeps a worktree that is not a live worktree, and one for each kept
/// worktree that still has a claim, since its work is no longer
/// where the item says it waits.
pub fn findings(loaded: &std::result::Result<Vec<Loaded>, String>) -> Result<Vec<Advice>> {
    let Ok(loaded) = loaded else {
        return Ok(Vec::new());
    };
    let mut findings = Vec::new();
    for project in loaded {
        let Ok(lists) = &project.lists else { continue };
        for item in &lists.items {
            for kept in &item.kept {
                if let Some(checkout) = crate::lists::running::live_worktree(&kept.worktree)? {
                    if let Some(claim) = crate::claim::Claim::read(&checkout.claim_file())? {
                        findings.push(Advice::run(
                            format!(
                                "lists: item {} \"{}\" keeps {}, which still has a claim ({}); remove the claim with",
                                item.id.0,
                                item.title,
                                kept.worktree,
                                claim.text()
                            ),
                            format!("builds item stop {} --why WHY", item.id.0),
                        ));
                    }
                    continue;
                }
                findings.push(Advice::run(
                    format!(
                        "lists: item {} \"{}\" keeps {}, which is not a live worktree; see what it holds with",
                        item.id.0, item.title, kept.worktree
                    ),
                    format!("builds item show {}", item.id.0),
                ));
            }
        }
    }
    Ok(findings)
}

/// An item and the worktree and branch of each place it keeps.
pub type KeptByItem = (Id, Vec<(String, String)>);

/// Makes `kept_work` and `kept_work_place` from the `parked` and
/// `parked_place` of a store from before builds named what an item keeps:
/// each parked item keeps the same worktrees, branches and moment. Returns
/// each item's place, so the move can say what it did. migrate.rs runs this
/// with the foreign keys off.
pub fn move_to_kept_work(db: &Connection) -> Result<Vec<KeptByItem>> {
    const SQL: &str = include_str!("kept_work.sql");
    let tables = crate::db::statement(SQL, "CREATE TABLE kept_work (", "STRICT;");
    let places = crate::db::statement(
        SQL,
        "CREATE TABLE kept_work_place (",
        "STRICT, WITHOUT ROWID;",
    );
    let index = crate::db::statement(SQL, "CREATE INDEX kept_work_place_worktree", ";");
    db.execute_batch(&format!(
        "{tables}\n{places}\n{index}
INSERT INTO kept_work (item_id, since_at, since_by) SELECT item_id, since_at, since_by FROM parked;
INSERT INTO kept_work_place (item_id, repo, parent, checkout, branch) SELECT item_id, repo, parent, checkout, branch FROM parked_place;"
    ))
    .sql()?;
    let mut statement = db
        .prepare("SELECT item_id, worktree, branch FROM kept_work_place ORDER BY item_id, repo")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                (row.get::<_, String>(1)?, row.get::<_, String>(2)?),
            ))
        })
        .sql()?;
    let mut kept: Vec<KeptByItem> = Vec::new();
    for row in rows {
        let (item, place) = row.sql()?;
        match kept.last_mut() {
            Some((last, places)) if *last == item => places.push(place),
            _ => kept.push((item, vec![place])),
        }
    }
    drop(statement);
    db.execute_batch("DROP TABLE parked_place; DROP TABLE parked;")
        .sql()?;
    Ok(kept)
}

/// The worktrees and branches <places> name, as a message lists them:
/// `W on B and W2 on B2`.
pub fn places_words<'a>(places: impl IntoIterator<Item = (&'a String, &'a String)>) -> String {
    places
        .into_iter()
        .map(|(worktree, branch)| format!("{worktree} on {branch}"))
        .collect::<Vec<_>>()
        .join(" and ")
}
