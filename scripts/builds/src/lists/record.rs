//! What every record shares: the id space (one for questions, items, design
//! changes and decisions), the stamp of who and when, what a record is, where
//! it is (open or closed), and how it leaves its list. Closing a record adds
//! its `closing` row; the owners' own rows stay as they were (record.sql).

use std::collections::BTreeMap;
use std::fmt;

use rusqlite::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::{DesignChange, Item, Lists, Question};

/// A record's id, from one counter per project; printed `#N`. Commands take
/// it as a bare number, since an unquoted `#12` is a comment in the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub u32);

impl ToSql for Id {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for Id {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        u32::column_result(value).map(Id)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl Id {
    /// A bare number: `12`. `#12` is refused with the reason, since a shell
    /// never passes it on.
    pub fn parse(text: &str) -> Result<Id> {
        if let Some(bare) = text.strip_prefix('#') {
            fail!("ids are bare numbers, '{bare}', not '{text}' (an unquoted # starts a shell comment)");
        }
        match text.parse::<u32>() {
            Ok(n) if n >= 1 && text.bytes().all(|b| b.is_ascii_digit()) => Ok(Id(n)),
            _ => fail!("'{text}' is not an id: ids are whole numbers from 1"),
        }
    }

    /// `3,5,8`.
    pub fn parse_list(text: &str) -> Result<Vec<Id>> {
        text.split(',').map(Id::parse).collect()
    }
}

/// When and by whom: a write's moment (seconds since 1970) and the one who
/// wrote (`--by`, else the claim of the worktree the command ran in).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub at: i64,
    pub by: String,
}

/// How a record left its list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum How {
    Answered(Id),
    /// Settled from this rule or code, not by the person.
    Settled(String),
    Done,
    Landed(Vec<String>),
    Dropped(String),
    /// The open GitHub issue it moved to.
    Deferred(String),
    /// The question that took the item's place: the item was the person's.
    MovedToStack(Id),
}

/// A record that left its list, as it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Record {
    Question(Question),
    Item(Item),
    Design(DesignChange),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Closed {
    pub record: Record,
    pub how: How,
    pub closed: Stamp,
}

/// What a record is, open or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Question,
    Item,
    Design,
    Decision,
}

impl Kind {
    /// The word the database holds.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Question => "question",
            Kind::Item => "item",
            Kind::Design => "design",
            Kind::Decision => "decision",
        }
    }

    pub fn from_word(word: &str) -> Kind {
        match word {
            "question" => Kind::Question,
            "item" => Kind::Item,
            "design" => Kind::Design,
            "decision" => Kind::Decision,
            other => unreachable!("record_kind_known holds, but the database has {other:?}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Question => "question",
            Kind::Item => "item",
            Kind::Design => "design change",
            Kind::Decision => "decision",
        }
    }
}

/// Where a record is.
#[derive(Clone, Copy, Debug)]
pub enum Standing<'a> {
    Open,
    Closed(&'a Closed),
}

#[derive(Clone, Copy, Debug)]
pub struct Located<'a> {
    pub kind: Kind,
    pub standing: Standing<'a>,
    pub title: &'a str,
}

impl How {
    /// The word flow counts it under.
    pub fn label(&self) -> &'static str {
        match self {
            How::Answered(_) => "answered",
            How::Settled(_) => "settled",
            How::Done => "done",
            How::Landed(_) => "landed",
            How::Dropped(_) => "dropped",
            How::Deferred(_) => "deferred",
            How::MovedToStack(_) => "moved-to-stack",
        }
    }

    /// Can a record of <kind> leave this way?
    pub fn fits(&self, kind: Kind) -> bool {
        match kind {
            Kind::Question => matches!(
                self,
                How::Answered(_) | How::Settled(_) | How::Done | How::Dropped(_) | How::Deferred(_)
            ),
            Kind::Item => matches!(
                self,
                How::Landed(_) | How::Dropped(_) | How::Deferred(_) | How::MovedToStack(_)
            ),
            Kind::Design => matches!(self, How::Landed(_)),
            Kind::Decision => false,
        }
    }
}

impl Record {
    pub fn id(&self) -> Id {
        match self {
            Record::Question(q) => q.id,
            Record::Item(i) => i.id,
            Record::Design(d) => d.id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Record::Question(q) => &q.title,
            Record::Item(i) => &i.title,
            Record::Design(d) => &d.title,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Record::Question(_) => Kind::Question,
            Record::Item(_) => Kind::Item,
            Record::Design(_) => Kind::Design,
        }
    }

    pub fn opened(&self) -> &Stamp {
        match self {
            Record::Question(q) => &q.opened,
            Record::Item(i) => &i.opened,
            Record::Design(d) => &d.opened,
        }
    }
}

impl Lists {
    pub fn closed_record(&self, id: Id) -> Option<&Closed> {
        self.closed.iter().find(|c| c.record.id() == id)
    }

    /// What <id> names and whether it is open or closed, or None when no
    /// record has it.
    pub fn locate(&self, id: Id) -> Option<Located<'_>> {
        if let Some(q) = self.question(id) {
            return Some(Located { kind: Kind::Question, standing: Standing::Open, title: &q.title });
        }
        if let Some(i) = self.item(id) {
            return Some(Located { kind: Kind::Item, standing: Standing::Open, title: &i.title });
        }
        if let Some(d) = self.design(id) {
            return Some(Located { kind: Kind::Design, standing: Standing::Open, title: &d.title });
        }
        if let Some(d) = self.decision(id) {
            return Some(Located { kind: Kind::Decision, standing: Standing::Open, title: &d.title });
        }
        let closed = self.closed_record(id)?;
        Some(Located { kind: closed.record.kind(), standing: Standing::Closed(closed), title: closed.record.title() })
    }
}

/// Opens a record of <kind>, made by <stamp>, and hands out its id: one more
/// than the highest there is.
pub fn open(db: &Connection, kind: Kind, stamp: &Stamp) -> Result<Id> {
    db.execute(
        "INSERT INTO record (kind, opened_at, opened_by) VALUES (?1, ?2, ?3)",
        params![kind.word(), stamp.at, stamp.by],
    )
    .sql()?;
    Ok(Id(db.last_insert_rowid() as u32))
}

/// What record <id> is, and how it left its list if it has.
pub fn standing(db: &Connection, id: Id) -> Result<Option<(Kind, Option<How>)>> {
    let Some(kind) = db
        .query_row("SELECT kind FROM record WHERE id = ?1", [id], |row| row.get::<_, String>(0))
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .sql()?
    else {
        return Ok(None);
    };
    let kind = Kind::from_word(&kind);
    let how = closing_of(db, id)?.map(|(how, _)| how);
    Ok(Some((kind, how)))
}

/// Closes open record <id> the way <how> says. Whether anything still waits
/// on it is the code check's to refuse when the write ends.
pub fn close(db: &Connection, id: Id, how: How, stamp: &Stamp) -> Result<()> {
    let kind = match standing(db, id)? {
        None => fail!("{id} is not a record"),
        Some((_, Some(closed))) => fail!("{id} is already closed ({})", closed.label()),
        Some((kind, None)) => kind,
    };
    if !how.fits(kind) {
        fail!("a {} cannot be closed as {}", kind.name(), how.label());
    }
    insert_closing(db, id, kind, &how, stamp)
}

/// Writes the closing of <id>, a <kind>, and its commits.
fn insert_closing(db: &Connection, id: Id, kind: Kind, how: &How, stamp: &Stamp) -> Result<()> {
    let (decision, question, reason) = match how {
        How::Answered(decision) => (Some(*decision), None, None),
        How::MovedToStack(question) => (None, Some(*question), None),
        How::Settled(reason) | How::Dropped(reason) | How::Deferred(reason) => (None, None, Some(reason.as_str())),
        How::Done | How::Landed(_) => (None, None, None),
    };
    db.execute(
        "INSERT INTO closing (record_id, kind, how, at, by, decision_id, question_id, reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, kind.word(), how.label(), stamp.at, stamp.by, decision, question, reason],
    )
    .sql()?;
    if let How::Landed(commits) = how {
        for (position, sha) in commits.iter().enumerate() {
            db.execute(
                "INSERT INTO closing_commit (record_id, how, position, sha) VALUES (?1, 'landed', ?2, ?3)",
                params![id, position as i64, sha],
            )
            .sql()?;
        }
    }
    Ok(())
}

/// How <id> left its list and who closed it when, or None when it is open.
fn closing_of(db: &Connection, id: Id) -> Result<Option<(How, Stamp)>> {
    Ok(read_closings(db)?.remove(&id))
}

/// Every closing, by the id of the record it closed.
pub fn read_closings(db: &Connection) -> Result<BTreeMap<Id, (How, Stamp)>> {
    let mut commits: BTreeMap<Id, Vec<String>> = BTreeMap::new();
    let mut statement = db.prepare("SELECT record_id, sha FROM closing_commit ORDER BY record_id, position").sql()?;
    for row in statement.query_map([], |row| Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?))).sql()? {
        let (id, sha) = row.sql()?;
        commits.entry(id).or_default().push(sha);
    }
    let mut statement = db
        .prepare("SELECT record_id, how, at, by, decision_id, question_id, reason FROM closing ORDER BY record_id")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                Stamp { at: row.get(2)?, by: row.get(3)? },
                row.get::<_, Option<Id>>(4)?,
                row.get::<_, Option<Id>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .sql()?;
    let mut all = BTreeMap::new();
    for row in rows {
        let (id, how, stamp, decision, question, reason) = row.sql()?;
        let reason = || reason.clone().expect("closing_reason_goes_with_how holds");
        let how = match how.as_str() {
            "answered" => How::Answered(decision.expect("closing_answered_names_its_decision holds")),
            "settled" => How::Settled(reason()),
            "done" => How::Done,
            "landed" => How::Landed(commits.remove(&id).unwrap_or_default()),
            "dropped" => How::Dropped(reason()),
            "deferred" => How::Deferred(reason()),
            "moved-to-stack" => How::MovedToStack(question.expect("closing_moved_names_its_question holds")),
            other => unreachable!("closing_how_known holds, but the database has {other:?}"),
        };
        all.insert(id, (how, stamp));
    }
    Ok(all)
}

/// Every record's opened stamp, by id.
pub fn read_opened(db: &Connection) -> Result<BTreeMap<Id, Stamp>> {
    let mut statement = db.prepare("SELECT id, opened_at, opened_by FROM record").sql()?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, Id>(0)?, Stamp { at: row.get(1)?, by: row.get(2)? })))
        .sql()?;
    rows.map(|row| row.sql()).collect()
}

/// Puts record <id> in as it was written elsewhere: its kind and who opened
/// it when.
pub fn move_in_record(db: &Connection, id: Id, kind: Kind, opened: &Stamp) -> Result<()> {
    db.execute(
        "INSERT INTO record (id, kind, opened_at, opened_by) VALUES (?1, ?2, ?3, ?4)",
        params![id, kind.word(), opened.at, opened.by],
    )
    .sql()?;
    Ok(())
}

/// Puts the closing of <id>, a <kind>, in as it was written elsewhere.
pub fn move_in_closing(db: &Connection, id: Id, kind: Kind, how: &How, closed: &Stamp) -> Result<()> {
    insert_closing(db, id, kind, how, closed)
}

/// The tables record.rs owns.
#[cfg(test)]
const TABLES: [&str; 3] = ["record", "closing", "closing_commit"];

/// Writes a row of one of record.rs's tables as given, the constraints still
/// holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(db: &Connection, table: &str, columns: &[(&str, rusqlite::types::Value)]) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
