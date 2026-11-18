//! Publishing a project's designs to claude.ai from the dashboard. Only a
//! Claude session can call the Artifact tool, so the store holds the request
//! and the session's inbox only wakes it: a design session registers its
//! inbox for the project (`builds design listen`), the dashboard's Publish
//! button records a request for one commit and sends that inbox a message
//! naming it (`ask_and_tell`), the session takes the request (`builds design
//! publish take N`), publishes with the publish-designs skill, and records
//! how it ended: `builds design publish done N`, after the design system's
//! last calls and the check of the canvas runtime, records that it published
//! the request's commit, and `builds design publish failed N --why` records
//! what stopped it. The tables are `publish.sql`; this is the only code that
//! writes them. The listener's token is a per-session secret, kept as
//! `design_session.rs` keeps a recording session's.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::Stamp;
use crate::lists::design_session::{Messaging, Sent, send};
use crate::lists::store::Store;

/// A request to publish the project's designs from one commit, as the
/// dashboard shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Request {
    pub id: i64,
    /// The full id of the commit to publish.
    pub commit: String,
    pub asked: Stamp,
    /// The session that took it, and when.
    pub taken: Option<Stamp>,
    pub ended: Option<Ended>,
}

/// How a request ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ended {
    pub at: i64,
    /// None when it published the commit, else what stopped it.
    pub error: Option<String>,
}

/// The error when no design session listens for <project>'s publish
/// requests.
pub fn no_listener(project: &str) -> String {
    format!("No design session is listening for {project}")
}

/// The words the listening session is sent for request <id>.
pub fn asked_words(project: &str, id: i64, commit: &str) -> String {
    format!(
        "Publish request #{id} of project {project}: publish the designs at commit {commit}. Take it with `builds design publish take {id} --project {project}`, then follow the publish-designs skill."
    )
}

/// Keeps <to> as the inbox of the session that listens for the project's
/// publish requests, in place of any earlier one.
pub fn listen(db: &Connection, to: &Messaging, stamp: &Stamp) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO design_listener (singleton, socket, token, at, by) VALUES (1, ?1, ?2, ?3, ?4)",
        params![to.socket(), to.token(), stamp.at, stamp.by],
    )
    .sql()?;
    Ok(())
}

fn listener(db: &Connection) -> Result<Option<Messaging>> {
    let row: Option<(String, Option<String>)> = db
        .query_row("SELECT socket, token FROM design_listener", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .sql()?;
    row.map(|(socket, token)| Messaging::new(&socket, token.as_deref()))
        .transpose()
}

const COLUMNS: &str =
    "id, commit_id, asked_at, asked_by, taken_at, taken_by, ended_at, outcome, error";

fn request_of(row: &rusqlite::Row) -> rusqlite::Result<Request> {
    let stamp = |at: Option<i64>, by: Option<String>| at.zip(by).map(|(at, by)| Stamp { at, by });
    let ended_at: Option<i64> = row.get(6)?;
    Ok(Request {
        id: row.get(0)?,
        commit: row.get(1)?,
        asked: Stamp {
            at: row.get(2)?,
            by: row.get(3)?,
        },
        taken: stamp(row.get(4)?, row.get(5)?),
        ended: match ended_at {
            Some(at) => Some(Ended {
                at,
                error: row.get(8)?,
            }),
            None => None,
        },
    })
}

/// Request <id>; fails when there is none.
fn request(db: &Connection, id: i64) -> Result<Request> {
    let found = db
        .query_row(
            &format!("SELECT {COLUMNS} FROM publish_request WHERE id = ?1"),
            [id],
            request_of,
        )
        .optional()
        .sql()?;
    match found {
        Some(request) => Ok(request),
        None => fail!("there is no publish request {id}"),
    }
}

/// What the dashboard shows of the project's publishing: the newest request,
/// and the newest that published.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Standing {
    pub latest: Option<Request>,
    pub published: Option<Request>,
}

/// The project's publishing as <db> holds it.
pub fn standing(db: &Connection) -> Result<Standing> {
    let newest = |filter: &str| {
        db.query_row(
            &format!("SELECT {COLUMNS} FROM publish_request {filter} ORDER BY id DESC LIMIT 1"),
            [],
            request_of,
        )
        .optional()
        .sql()
    };
    Ok(Standing {
        latest: newest("")?,
        published: newest("WHERE outcome = 'published'")?,
    })
}

/// Records a request to publish <commit> (a full commit id), asked by
/// <stamp>, and returns it with the listening session's inbox. Fails when a
/// request is open or no session listens for the project.
fn ask(
    db: &Connection,
    project: &str,
    commit: &str,
    stamp: &Stamp,
) -> Result<(Request, Messaging)> {
    if let Some(open) = db
        .query_row(
            "SELECT id FROM publish_request WHERE ended_at IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .sql()?
    {
        fail!("publish request {open} is still open; wait until it ends");
    }
    let Some(to) = listener(db)? else {
        fail!("{}", no_listener(project));
    };
    db.execute(
        "INSERT INTO publish_request (commit_id, asked_at, asked_by) VALUES (?1, ?2, ?3)",
        params![commit, stamp.at, stamp.by],
    )
    .sql()?;
    Ok((request(db, db.last_insert_rowid())?, to))
}

/// Ends open request <id> at <at>, published or with <error>.
fn end(db: &Connection, id: i64, at: i64, error: Option<&str>) -> Result<()> {
    let outcome = if error.is_some() {
        "failed"
    } else {
        "published"
    };
    db.execute(
        "UPDATE publish_request SET ended_at = ?2, outcome = ?3, error = ?4 WHERE id = ?1",
        params![id, at, outcome, error],
    )
    .sql()?;
    Ok(())
}

/// Records a request to publish <commit> in <store> and sends the listening
/// session a message naming it, once. When nothing listens on the inbox,
/// the request ends failed with `no_listener`, and that is the error; any
/// other failure of the send is the error too, with the request open and
/// the session not told, and nothing retries it.
pub fn ask_and_tell(store: &Store, commit: &str, stamp: &Stamp) -> Result<Request> {
    let project = store.project.clone();
    let (asked, to) = store.write(stamp, &format!("design publish ask {commit}"), |db| {
        ask(db, &project, commit, stamp)
    })?;
    match send(&to, &asked_words(&project, asked.id, commit))? {
        Sent::Delivered => Ok(asked),
        Sent::Gone => {
            let why = no_listener(&project);
            store.write(
                stamp,
                &format!("design publish failed {}", asked.id),
                |db| end(db, asked.id, stamp.at, Some(&why)),
            )?;
            fail!("{why}")
        }
    }
}

/// Records that <stamp>'s session took open request <id>.
pub fn take(db: &Connection, id: i64, stamp: &Stamp) -> Result<Request> {
    let request = request(db, id)?;
    if request.ended.is_some() {
        fail!("publish request {id} has ended");
    }
    if let Some(taken) = &request.taken {
        fail!("{} already took publish request {id}", taken.by);
    }
    db.execute(
        "UPDATE publish_request SET taken_at = ?2, taken_by = ?3 WHERE id = ?1",
        params![id, stamp.at, stamp.by],
    )
    .sql()?;
    self::request(db, id)
}

/// Records that taken request <id> published its commit at <at>, which ends
/// it.
pub fn published(db: &Connection, id: i64, at: i64) -> Result<Request> {
    let request = request(db, id)?;
    if request.ended.is_some() {
        fail!("publish request {id} has ended");
    }
    if request.taken.is_none() {
        fail!(
            "publish request {id} was never taken; take it with `builds design publish take {id}` first"
        );
    }
    end(db, id, at, None)?;
    self::request(db, id)
}

/// Records that open request <id> failed at <at> for <why>, which ends it.
pub fn failed(db: &Connection, id: i64, why: &str, at: i64) -> Result<()> {
    if why.trim().is_empty() {
        fail!("say what stopped the publish with --why");
    }
    if request(db, id)?.ended.is_some() {
        fail!("publish request {id} has ended");
    }
    end(db, id, at, Some(why))
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
