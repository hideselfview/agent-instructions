//! Items of other projects that an item waits on (`Wait::Foreign`). Each
//! project's store holds its own items, so a wait on one cannot be a row that
//! points at it. A load of a store reads the item's standing from the other
//! project's store and keeps it in `Lists::foreign`, so every reader of the
//! lists sees the wait met or unmet without opening another store itself.

use std::path::Path;

use rusqlite::Connection;

use crate::db::{self, DATABASE, Sql};
use crate::fail;
use crate::fail::{Fail, Result};
use crate::lists::check::project_name_fault;
use crate::lists::record::{self, How, Id, Kind, Record};
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
}

/// What reading an item of another project found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForeignState {
    Read {
        title: String,
        standing: ForeignStanding,
    },
    /// The store or the item could not be read: why.
    Unreadable(String),
}

/// An item of another project that an item of this one waits on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForeignItem {
    pub project: String,
    pub id: Id,
    pub state: ForeignState,
}

use serde::Serialize;

impl ForeignItem {
    /// <project>'s item <id> as the store in folder <lists> holds it now.
    pub fn read(lists: &Path, project: &str, id: Id) -> ForeignItem {
        let state = match read_standing(lists, project, id) {
            Ok((title, standing)) => ForeignState::Read { title, standing },
            Err(error) => ForeignState::Unreadable(error.text()),
        };
        ForeignItem {
            project: project.to_string(),
            id,
            state,
        }
    }

    /// An item whose reading failed at once, for <why>.
    fn unreadable(project: &str, id: Id, why: String) -> ForeignItem {
        ForeignItem {
            project: project.to_string(),
            id,
            state: ForeignState::Unreadable(why),
        }
    }

    pub fn title(&self) -> Option<&str> {
        match &self.state {
            ForeignState::Read { title, .. } => Some(title),
            ForeignState::Unreadable(_) => None,
        }
    }

    pub fn standing(&self) -> Option<&ForeignStanding> {
        match &self.state {
            ForeignState::Read { standing, .. } => Some(standing),
            ForeignState::Unreadable(_) => None,
        }
    }

    pub fn met(&self) -> bool {
        matches!(self.standing(), Some(ForeignStanding::Met(_)))
    }

    /// Does a wait on it still hold something? Not once it landed or closed
    /// without meeting waits; an item that could not be read may still.
    pub fn holds(&self) -> bool {
        !matches!(
            self.standing(),
            Some(ForeignStanding::Met(_) | ForeignStanding::Ended(_))
        )
    }

    /// What the state says, for a person.
    pub fn words(&self) -> String {
        match &self.state {
            ForeignState::Read { standing, .. } => match standing {
                ForeignStanding::Open => "open".to_string(),
                ForeignStanding::Backlog => "in the backlog".to_string(),
                ForeignStanding::Met(_) => "landed".to_string(),
                ForeignStanding::Ended(how) => {
                    format!("closed as {how}, which meets no wait: end the wait by hand")
                }
            },
            ForeignState::Unreadable(why) => format!("unreadable: {why}"),
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
            if crate::lists::item::in_backlog(conn, id)? {
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

/// The folder that holds every project's store, from the path of one
/// project's database: <lists>/<project>/builds.sqlite.
fn folder_of(database: Option<&Path>) -> Result<&Path> {
    let Some(database) = database else {
        fail!(
            "the lists were read from a database with no file, so builds cannot find the other projects' stores"
        );
    };
    match database.parent().and_then(Path::parent) {
        Some(folder) => Ok(folder),
        None => fail!(
            "{} is not in a project's folder, so builds cannot find the other projects' stores",
            database.display()
        ),
    }
}

/// Every item of another project that an item of <lists>, open or closed,
/// waits on, read from the stores beside <database>, the database <lists>
/// came from (None for one with no file), in project then id order.
pub fn resolve(database: Option<&Path>, lists: &Lists) -> Vec<ForeignItem> {
    let closed = lists
        .closed
        .iter()
        .filter_map(|closed| match &closed.record {
            Record::Item(item) => Some(item),
            _ => None,
        });
    let mut named: Vec<(&str, Id)> = lists
        .items
        .iter()
        .chain(closed)
        .flat_map(|item| &item.waits)
        .filter_map(|wait| match wait {
            Wait::Foreign { project, item } => Some((project.as_str(), *item)),
            _ => None,
        })
        .collect();
    named.sort();
    named.dedup();
    let folder = folder_of(database).map_err(Fail::text);
    named
        .into_iter()
        .map(|(project, id)| match &folder {
            Ok(folder) => ForeignItem::read(folder, project, id),
            Err(why) => ForeignItem::unreadable(project, id, why.clone()),
        })
        .collect()
}

impl Lists {
    /// The item <id> of project <project> that a wait of this project's items
    /// names, which loading the lists read. Panics when it is missing: a
    /// `Lists` holds every item its waits name, an unreadable one as
    /// `ForeignState::Unreadable`.
    pub fn foreign_item_read(&self, project: &str, id: Id) -> &ForeignItem {
        self.foreign_item(project, id).unwrap_or_else(|| {
            panic!(
                "the lists hold no reading of item {id} of project {project}, which a wait names"
            )
        })
    }

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
    let ForeignState::Read { standing, .. } = &found.state else {
        fail!(
            "a wait names {item} of project {project}: {}",
            found.words()
        );
    };
    match standing {
        ForeignStanding::Open => Ok(()),
        ForeignStanding::Backlog => fail!(
            "a wait names {item} of project {project}, which is in that project's backlog: start it first (`builds item start {} --project {project} --said WORDS`)",
            item.0
        ),
        ForeignStanding::Met(_) | ForeignStanding::Ended(_) => fail!(
            "a wait names {item} of project {project}, which is closed ({}); a wait on a closed item never holds anything",
            found.words()
        ),
    }
}

#[cfg(test)]
#[path = "item_wait_foreign_tests.rs"]
mod tests;
