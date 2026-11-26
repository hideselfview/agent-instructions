//! Items of other projects that an item waits on (`Wait::Foreign`). Each
//! project's store holds its own items, so a wait on one cannot be a row that
//! points at it. A load of a store reads the item's standing from the other
//! project's store and keeps it in `Lists::foreign`, so every reader of the
//! lists sees the wait met or unmet without opening another store itself.

use std::path::Path;

use rusqlite::Connection;

use crate::db::{self, DATABASE, Sql};
use crate::fail;
use crate::fail::Result;
use crate::lists::check::project_name_fault;
use crate::lists::record::{self, How, Id, Kind};
use crate::lists::{Lists, Wait};

/// Writes item <id>'s wait on item <item> of <project>, once.
pub fn insert(db: &Connection, id: Id, project: &str, item: Id) -> rusqlite::Result<usize> {
    db.execute(
        "INSERT OR IGNORE INTO item_wait_foreign (item_id, project, on_item) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, project, item],
    )
}

/// Ends item <id>'s wait on item <item> of <project>; the rows removed.
pub fn delete(db: &Connection, id: Id, project: &str, item: Id) -> rusqlite::Result<usize> {
    db.execute(
        "DELETE FROM item_wait_foreign WHERE item_id = ?1 AND project = ?2 AND on_item = ?3",
        rusqlite::params![id, project, item],
    )
}

/// Every item's waits on items of other projects, as (item, wait).
pub fn read(db: &Connection) -> Result<Vec<(Id, Wait)>> {
    let mut statement = db
        .prepare(
            "SELECT item_id, project, on_item FROM item_wait_foreign ORDER BY project, on_item, item_id",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                Wait::Foreign {
                    project: row.get(1)?,
                    item: row.get(2)?,
                },
            ))
        })
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// Where an item of another project stands, as that project's store holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForeignStanding {
    Open,
    /// Open, in the project's backlog: nobody has started it.
    Backlog,
    /// Landed or finished at this time: a wait on it is met.
    Met(i64),
    /// Closed in a way that meets no wait (the word `How::label` gives): only
    /// `builds item wait --off` ends a wait on it.
    Ended(String),
    /// The store or the item could not be read: why.
    Unreadable(String),
}

/// An item of another project that an item of this one waits on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForeignItem {
    pub project: String,
    pub id: Id,
    /// Empty when the item could not be read.
    pub title: String,
    pub standing: ForeignStanding,
}

use serde::Serialize;

impl ForeignItem {
    /// <project>'s item <id> as the store in folder <lists> holds it now.
    pub fn read(lists: &Path, project: &str, id: Id) -> ForeignItem {
        let (title, standing) = match read_standing(lists, project, id) {
            Ok(found) => found,
            Err(error) => (String::new(), ForeignStanding::Unreadable(error.text())),
        };
        ForeignItem {
            project: project.to_string(),
            id,
            title,
            standing,
        }
    }

    pub fn met(&self) -> bool {
        matches!(self.standing, ForeignStanding::Met(_))
    }

    /// What the standing says, for a person.
    pub fn words(&self) -> String {
        match &self.standing {
            ForeignStanding::Open => "open".to_string(),
            ForeignStanding::Backlog => "in the backlog".to_string(),
            ForeignStanding::Met(_) => "landed".to_string(),
            ForeignStanding::Ended(how) => {
                format!("closed as {how}, which meets no wait: end the wait by hand")
            }
            ForeignStanding::Unreadable(why) => format!("unreadable: {why}"),
        }
    }
}

fn read_standing(lists: &Path, project: &str, id: Id) -> Result<(String, ForeignStanding)> {
    if let Some(fault) = project_name_fault(project) {
        fail!("{fault}");
    }
    let database = lists.join(project).join(DATABASE);
    let Some(conn) = db::open_reader(&database)? else {
        fail!("project {project} has no lists");
    };
    standing_in(&conn, project, id)
}

fn standing_in(conn: &Connection, project: &str, id: Id) -> Result<(String, ForeignStanding)> {
    let title: Option<String> = rusqlite::OptionalExtension::optional(conn.query_row(
        "SELECT title FROM item WHERE id = ?1",
        [id],
        |row| row.get(0),
    ))
    .sql()?;
    let Some(title) = title else {
        fail!("project {project} has no item {id}");
    };
    let standing = match record::standing(conn, id)? {
        Some((Kind::Item, None)) => {
            let backlog: bool = conn
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM item_backlog WHERE item_id = ?1)",
                    [id],
                    |row| row.get(0),
                )
                .sql()?;
            if backlog {
                ForeignStanding::Backlog
            } else {
                ForeignStanding::Open
            }
        }
        Some((Kind::Item, Some(How::Landed(_) | How::Finished(_)))) => {
            let at: i64 = conn
                .query_row("SELECT at FROM closing WHERE record_id = ?1", [id], |row| {
                    row.get(0)
                })
                .sql()?;
            ForeignStanding::Met(at)
        }
        Some((Kind::Item, Some(how))) => ForeignStanding::Ended(how.label().to_string()),
        _ => fail!("project {project} has no item {id}"),
    };
    Ok((title, standing))
}

/// Every item of another project that an open item of <lists> waits on, read
/// from the stores in folder <folder>, in project then id order.
pub fn resolve(folder: &Path, lists: &Lists) -> Vec<ForeignItem> {
    let mut named: Vec<(&str, Id)> = lists
        .items
        .iter()
        .flat_map(|item| &item.waits)
        .filter_map(|wait| match wait {
            Wait::Foreign { project, item } => Some((project.as_str(), *item)),
            _ => None,
        })
        .collect();
    named.sort();
    named.dedup();
    named
        .into_iter()
        .map(|(project, id)| ForeignItem::read(folder, project, id))
        .collect()
}

impl Lists {
    /// The item <id> of project <project> that an item waits on.
    pub fn foreign_item(&self, project: &str, id: Id) -> Option<&ForeignItem> {
        self.foreign
            .iter()
            .find(|found| found.project == project && found.id == id)
    }
}

/// Refuses a wait on an item of another project that cannot be made now: one
/// in the item's own project, which is `item:N`, one that the other project
/// does not hold, one closed, or one in its backlog. <own> is the waiting
/// item's project and <folder> holds every project's store.
pub fn require_open(folder: &Path, own: &str, wait: &Wait) -> Result<()> {
    let Wait::Foreign { project, item } = wait else {
        return Ok(());
    };
    if project == own {
        fail!(
            "{project} is this item's own project: wait on item:{}",
            item.0
        );
    }
    let found = ForeignItem::read(folder, project, *item);
    match found.standing {
        ForeignStanding::Open => Ok(()),
        ForeignStanding::Backlog => fail!(
            "a wait names {item} of project {project}, which is in that project's backlog: start it first (`builds item start {} --project {project} --said WORDS`)",
            item.0
        ),
        ForeignStanding::Met(_) | ForeignStanding::Ended(_) => fail!(
            "a wait names {item} of project {project}, which is closed ({}); a wait on a closed item never holds anything",
            found.words()
        ),
        ForeignStanding::Unreadable(why) => {
            fail!("a wait names {item} of project {project}: {why}")
        }
    }
}

#[cfg(test)]
#[path = "item_wait_foreign_tests.rs"]
mod tests;
