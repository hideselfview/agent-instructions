//! Publishing a project's designs to claude.ai from the dashboard. Only a
//! Claude session can call the Artifact tool, so the store holds the request
//! and the session's inbox only wakes it: a design session registers its
//! inbox for the project (`builds design listen`), the dashboard's Publish
//! button records a request for one commit and sends that inbox a message
//! naming it (`ask_and_tell`), the session takes the request (`builds design
//! publish take N`), publishes with the publish-designs skill, and records
//! how it ended: `builds design publish done N --link TITLE=URL`, after the design system's
//! last calls and the check of the canvas runtime, records that it published
//! the request's commit, and `builds design publish failed N --why` records
//! what stopped it. The tables are `publish.sql`; this is the only code that
//! writes them. The listener's token is a per-session secret (inbox.rs).

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::inbox::{Messaging, Sent, Sessions, StoredEndpoint};
use crate::lists::Stamp;
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
    pub delivery: Option<Delivery>,
}

/// What builds knows about the notification while a request awaits pickup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    Unconfirmed,
    Delivered,
    Queued,
}

impl Delivery {
    fn sql(self) -> &'static str {
        match self {
            Self::Unconfirmed => "unconfirmed",
            Self::Delivered => "delivered",
            Self::Queued => "queued",
        }
    }
}

impl rusqlite::types::FromSql for Delivery {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        match value.as_str()? {
            "unconfirmed" => Ok(Self::Unconfirmed),
            "delivered" => Ok(Self::Delivered),
            "queued" => Ok(Self::Queued),
            other => Err(rusqlite::types::FromSqlError::Other(
                format!("unknown publish delivery {other:?}").into(),
            )),
        }
    }
}

/// One artifact a request published to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Link {
    pub title: String,
    pub url: String,
}

impl Link {
    /// The link that `TITLE=URL` gives.
    pub fn parse(text: &str) -> Result<Link> {
        let Some((title, url)) = text.rsplit_once('=') else {
            fail!("{text:?} is no link: write it as TITLE=URL");
        };
        if title.trim().is_empty() || !url.starts_with("https://") {
            fail!("{text:?} is no link: write it as TITLE=URL, with an https URL");
        }
        Ok(Link {
            title: title.trim().to_string(),
            url: url.to_string(),
        })
    }
}

/// How a request ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum Ended {
    /// The session published the commit to these artifacts, in the order it
    /// gave them.
    Published { at: i64, links: Vec<Link> },
    /// Something stopped the request, which `error` says.
    Failed { at: i64, error: String },
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
    let (runtime, address, token) = to.stored();
    db.execute(
        "INSERT OR REPLACE INTO design_listener (singleton, runtime, address, token, at, by) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![runtime, address, token, stamp.at, stamp.by],
    )
    .sql()?;
    Ok(())
}

fn listener(db: &Connection) -> Result<Option<Messaging>> {
    let row: Option<StoredEndpoint> = db
        .query_row(
            "SELECT runtime, address, token FROM design_listener",
            [],
            StoredEndpoint::read,
        )
        .optional()
        .sql()?;
    row.map(StoredEndpoint::restore).transpose()
}

const COLUMNS: &str =
    "id, commit_id, asked_at, asked_by, taken_at, taken_by, ended_at, outcome, error, delivery";

fn request_of(row: &rusqlite::Row) -> rusqlite::Result<Request> {
    let stamp = |at: Option<i64>, by: Option<String>| at.zip(by).map(|(at, by)| Stamp { at, by });
    let ended_at: Option<i64> = row.get(6)?;
    Ok(Request {
        id: row.get(0)?,
        delivery: row.get(9)?,
        commit: row.get(1)?,
        asked: Stamp {
            at: row.get(2)?,
            by: row.get(3)?,
        },
        taken: stamp(row.get(4)?, row.get(5)?),
        ended: match (ended_at, row.get::<_, Option<String>>(8)?) {
            (Some(at), Some(error)) => Some(Ended::Failed { at, error }),
            (Some(at), None) => Some(Ended::Published {
                at,
                links: Vec::new(),
            }),
            (None, _) => None,
        },
    })
}

/// <request> with the links it published to, when it published.
fn with_links(db: &Connection, mut request: Request) -> Result<Request> {
    let Some(Ended::Published { links, .. }) = &mut request.ended else {
        return Ok(request);
    };
    let mut statement = db
        .prepare("SELECT title, url FROM publish_link WHERE request_id = ?1 ORDER BY position")
        .sql()?;
    *links = statement
        .query_map([request.id], |row| {
            Ok(Link {
                title: row.get(0)?,
                url: row.get(1)?,
            })
        })
        .sql()?
        .collect::<rusqlite::Result<_>>()
        .sql()?;
    Ok(request)
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
        Some(request) => with_links(db, request),
        None => fail!("there is no publish request {id}"),
    }
}

/// What the dashboard shows of the project's publishing: the newest request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Standing {
    pub latest: Option<Request>,
}

/// The project's publishing as <db> holds it.
pub fn standing(db: &Connection) -> Result<Standing> {
    let latest = db
        .query_row(
            &format!("SELECT {COLUMNS} FROM publish_request ORDER BY id DESC LIMIT 1"),
            [],
            request_of,
        )
        .optional()
        .sql()?
        .map(|request| with_links(db, request))
        .transpose()?;
    Ok(Standing { latest })
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
    let asked = db.query_row(
        &format!("INSERT INTO publish_request (commit_id, asked_at, asked_by, delivery) VALUES (?1, ?2, ?3, 'unconfirmed') RETURNING {COLUMNS}"),
        params![commit, stamp.at, stamp.by],
        request_of,
    ).sql()?;
    Ok((asked, to))
}

/// Ends open request <id> at <at>, published or with <error>.
fn end(db: &Connection, id: i64, at: i64, error: Option<&str>) -> Result<()> {
    let outcome = if error.is_some() {
        "failed"
    } else {
        "published"
    };
    db.execute(
        "UPDATE publish_request SET ended_at = ?2, outcome = ?3, error = ?4, delivery = NULL WHERE id = ?1",
        params![id, at, outcome, error],
    )
    .sql()?;
    Ok(())
}

/// Records a request to publish <commit> in <store> and sends the listening
/// session one message naming it. The request can be taken before builds
/// confirms notification. A failed send ends a request that nobody took.
pub fn ask_and_tell(
    store: &Store,
    sessions: &Sessions,
    commit: &str,
    stamp: &Stamp,
) -> Result<Request> {
    let project = &store.project;
    let (asked, to) = store.write(stamp, &format!("design publish ask {commit}"), |db| {
        ask(db, project, commit, stamp)
    })?;
    let id = asked.id;
    let delivery = match sessions.send(&to, &asked_words(project, id, commit)) {
        Ok(Sent::Delivered) => Ok(Delivery::Delivered),
        Ok(Sent::Queued) => Ok(Delivery::Queued),
        Ok(Sent::Gone) => Err(crate::fail::Fail::Message(no_listener(project))),
        Err(error) => Err(error),
    };
    match delivery {
        Ok(delivery) => store.write(stamp, &format!("design publish notified {id}"), |db| {
            let updated = db.query_row(
                &format!("UPDATE publish_request SET delivery = CASE WHEN taken_at IS NULL AND ended_at IS NULL THEN ?2 ELSE NULL END WHERE id = ?1 RETURNING {COLUMNS}"),
                params![id, delivery.sql()],
                request_of,
            ).sql()?;
            with_links(db, updated)
        }),
        Err(error) => {
            let why = error.text();
            store.write(stamp, &format!("design publish failed {id}"), |db| {
                let ended = db.execute(
                    "UPDATE publish_request SET ended_at = ?2, outcome = 'failed', error = ?3, delivery = NULL WHERE id = ?1 AND taken_at IS NULL AND ended_at IS NULL",
                    params![id, stamp.at, why],
                ).sql()?;
                if ended == 0 {
                    return request(db, id).map(Some);
                }
                Ok(None)
            })
            .and_then(|completed| match completed {
                Some(completed) => {
                    crate::text::note(format!("Builds could not confirm notification of publish request {id}: {why}"));
                    Ok(completed)
                }
                None => fail!("{why}"),
            })
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
        "UPDATE publish_request SET taken_at = ?2, taken_by = ?3, delivery = NULL WHERE id = ?1",
        params![id, stamp.at, stamp.by],
    )
    .sql()?;
    self::request(db, id)
}

/// Records that taken request <id> published its commit to <links> at <at>,
/// which ends it.
pub fn published(db: &Connection, id: i64, links: &[Link], at: i64) -> Result<Request> {
    let request = request(db, id)?;
    if request.ended.is_some() {
        fail!("publish request {id} has ended");
    }
    if request.taken.is_none() {
        fail!(
            "publish request {id} was never taken; take it with `builds design publish take {id}` first"
        );
    }
    if links.is_empty() {
        fail!("say which artifacts the publish sent the designs to with --link TITLE=URL");
    }
    end(db, id, at, None)?;
    for (position, link) in links.iter().enumerate() {
        db.execute(
            "INSERT INTO publish_link (request_id, outcome, position, title, url) VALUES (?1, 'published', ?2, ?3, ?4)",
            params![id, position as i64, link.title, link.url],
        )
        .sql()?;
    }
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

pub fn move_legacy_destination(db: &Connection) -> Result<()> {
    use crate::db::{self, SCHEMA};
    crate::inbox::move_legacy_destination(
        db,
        "design_listener",
        db::statement(SCHEMA, "CREATE TABLE design_listener (", ") STRICT;"),
        "singleton",
        ", at, by",
    )?;
    db::remake_table(
        db,
        "publish_request",
        db::statement(SCHEMA, "CREATE TABLE publish_request (", ") STRICT;"),
        |db| {
            db.execute_batch(&format!("INSERT INTO publish_request ({COLUMNS}) SELECT id, commit_id, asked_at, asked_by, taken_at, taken_by, ended_at, outcome, error, CASE WHEN taken_at IS NULL AND ended_at IS NULL THEN 'unconfirmed' ELSE NULL END FROM publish_request_old;")).sql()
        },
        &[db::statement(
            SCHEMA,
            "CREATE UNIQUE INDEX publish_request_one_open",
            ";",
        )],
    )
}

#[cfg(test)]
pub(crate) use tests::PUBLISH_REQUEST;

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
