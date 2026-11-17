//! The decision stack's questions: what a question is, where it sits, and
//! every write to one, with the fates an answer gives the items that waited
//! on it. A question may name the record it came from (trail.rs).
//! `question_command` is the commands around these operations.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::item::ItemEdit;
use crate::lists::record::{self, close};
use crate::lists::{How, Id, Kind, Lists, Stamp, Wait};
use crate::lists::{store, trail};

impl Lists {
    pub fn question(&self, id: Id) -> Option<&Question> {
        self.questions.iter().find(|q| q.id == id)
    }
}

/// What a question is, and so what answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuestionKind {
    /// A question to answer.
    Question,
    /// Work only the person can do.
    YourStep,
    /// Work held until the person starts it.
    Held,
}

/// Where a placed question sits in the stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "at", rename_all = "lowercase")]
pub enum Position {
    /// In the person's ranked order at the top of the stack, above every
    /// question placed first with a lower rank.
    First { rank: i64 },
    /// At the bottom of the stack.
    Last,
}

impl Position {
    pub fn word(self) -> &'static str {
        match self {
            Position::First { .. } => "first",
            Position::Last => "last",
        }
    }
}

/// Where a question sits in the stack when not by how much it unblocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Placement {
    #[serde(flatten)]
    pub at: Position,
    pub why: String,
    pub by_person: bool,
}

/// Where `place` puts a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    /// On top of the ranked order, above every question placed there before.
    First,
    /// In the ranked order, directly above this question.
    Above(Id),
    /// In the ranked order, directly below this question.
    Below(Id),
    /// At the bottom of the stack.
    Last,
}

/// A plan's numbered open question or decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRef {
    pub path: String,
    pub number: String,
}

/// An entry of the decision stack. What it blocks is not stored: it is the
/// open items whose waits name it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Question {
    pub id: Id,
    pub title: String,
    pub details: String,
    pub kind: QuestionKind,
    /// The reason the question stays open though no open item waits on it;
    /// None while one does.
    pub blocks_nothing: Option<String>,
    pub placed: Option<Placement>,
    pub plan: Option<PlanRef>,
    /// The record it came from (trail.rs).
    pub origin: Option<Id>,
    pub opened: Stamp,
    /// The audits or groups it came from (tag.rs), in the order of their text.
    pub tags: Vec<String>,
}
/// What an answer does to one item that waited on the question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocked {
    /// The item goes on as it is; its wait is now met.
    Proceeds,
    Edit(ItemEdit),
    /// The item closes, for this reason.
    Drop(String),
}

/// For each item that waits on a question, what its answer does to it.
pub type BlockedMap = BTreeMap<Id, Blocked>;

/// A question as it is asked: everything but its id and who asked.
pub struct NewQuestion {
    pub title: String,
    pub details: String,
    pub kind: QuestionKind,
    /// Why it is asked though nothing waits on it.
    pub blocks_nothing: Option<String>,
    /// Why it goes on top of the ranked order, when it does.
    pub first: Option<String>,
    pub plan: Option<PlanRef>,
    /// The record it came from.
    pub origin: Option<Id>,
}

fn kind_word(kind: QuestionKind) -> &'static str {
    match kind {
        QuestionKind::Question => "question",
        QuestionKind::YourStep => "your-step",
        QuestionKind::Held => "held",
    }
}

/// The rank of question <id> in the ranked order, None when it is not
/// placed first.
fn rank_of(db: &Connection, id: Id) -> Result<Option<i64>> {
    db.query_row(
        "SELECT rank FROM question_placement WHERE question_id = ?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .sql()
    .map(Option::flatten)
}

/// Raises every rank from <from> up by one, the highest first, so no two
/// placements hold one rank at any step, and leaves rank <from> free.
fn free_rank(db: &Connection, from: i64) -> Result<()> {
    let mut statement = db
        .prepare("SELECT rank FROM question_placement WHERE rank >= ?1 ORDER BY rank DESC")
        .sql()?;
    let ranks = statement
        .query_map([from], |row| row.get::<_, i64>(0))
        .sql()?
        .collect::<rusqlite::Result<Vec<i64>>>()
        .sql()?;
    for rank in ranks {
        db.execute(
            "UPDATE question_placement SET rank = rank + 1 WHERE rank = ?1",
            [rank],
        )
        .sql()?;
    }
    Ok(())
}

/// Deletes question <id>'s placement, if it has one.
fn delete_placement(db: &Connection, id: Id) -> Result<()> {
    db.execute(
        "DELETE FROM question_placement WHERE question_id = ?1",
        [id],
    )
    .sql()?;
    Ok(())
}

/// Writes question <id>'s placement at <spot>, replacing one it has.
fn put(db: &Connection, id: Id, spot: Spot, why: &str, by_person: bool) -> Result<()> {
    // The question leaves its old place first, so a move within the ranked
    // order frees its old rank.
    delete_placement(db, id)?;
    let at = match spot {
        Spot::Last => Position::Last,
        Spot::First => {
            let top: i64 = db
                .query_row(
                    "SELECT COALESCE(MAX(rank), 0) FROM question_placement",
                    [],
                    |row| row.get(0),
                )
                .sql()?;
            Position::First { rank: top + 1 }
        }
        Spot::Above(next) | Spot::Below(next) => {
            if next == id {
                fail!("question {id} can't sit above or below itself");
            }
            require_open_question(db, next)?;
            let Some(rank) = rank_of(db, next)? else {
                fail!(
                    "question {next} is not in the ranked order: `builds question place {} first --why …` puts it there",
                    next.0
                )
            };
            let rank = if matches!(spot, Spot::Above(_)) {
                rank + 1
            } else {
                rank
            };
            free_rank(db, rank)?;
            Position::First { rank }
        }
    };
    let rank = match at {
        Position::First { rank } => Some(rank),
        Position::Last => None,
    };
    db.execute(
        "INSERT INTO question_placement (question_id, at, rank, why, by_person) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, at.word(), rank, why, by_person],
    )
    .sql()?;
    Ok(())
}

/// Asks <new> on the stack, opened by <stamp>; returns its id.
pub fn ask(db: &Connection, new: NewQuestion, stamp: &Stamp) -> Result<Id> {
    let id = record::open(db, Kind::Question, stamp)?;
    if let Some(origin) = new.origin {
        trail::require_record(db, origin)?;
    }
    insert(db, id, &new)?;
    Ok(id)
}

/// The question's own rows, for the record <id> already opened.
fn insert(db: &Connection, id: Id, new: &NewQuestion) -> Result<()> {
    db.execute(
        "INSERT INTO question (id, title, details, kind, blocks_nothing, origin) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, new.title, new.details, kind_word(new.kind), new.blocks_nothing, new.origin],
    )
    .sql()?;
    if let Some(why) = &new.first {
        put(db, id, Spot::First, why, false)?;
    }
    if let Some(plan) = &new.plan {
        db.execute(
            "INSERT INTO question_plan (question_id, path, number) VALUES (?1, ?2, ?3)",
            params![id, plan.path, plan.number],
        )
        .sql()?;
    }
    Ok(())
}

/// Question <id> is open, or the refusal.
fn require_open_question(db: &Connection, id: Id) -> Result<()> {
    match record::standing(db, id)? {
        Some((Kind::Question, None)) => Ok(()),
        _ => fail!("{id} is not an open question"),
    }
}

/// Places open question <id> at <spot>, for <why>, replacing a placement it
/// has. <by_person> says the person asked for the place.
pub fn place(db: &Connection, id: Id, spot: Spot, why: &str, by_person: bool) -> Result<()> {
    require_open_question(db, id)?;
    put(db, id, spot, why, by_person)
}

/// Removes open question <id>'s placement, so what it unblocks orders it.
pub fn unplace(db: &Connection, id: Id) -> Result<()> {
    require_open_question(db, id)?;
    delete_placement(db, id)
}

/// Changes open question <id>: the title, the details and the reason it
/// blocks nothing, each when given.
pub fn edit(
    db: &Connection,
    id: Id,
    title: Option<String>,
    details: Option<String>,
    blocks_nothing: Option<String>,
) -> Result<()> {
    require_open_question(db, id)?;
    db.execute(
        "UPDATE question SET title = COALESCE(?2, title), details = COALESCE(?3, details), \
         blocks_nothing = COALESCE(?4, blocks_nothing) WHERE id = ?1",
        params![id, title, details, blocks_nothing],
    )
    .sql()?;
    Ok(())
}

/// Clears the reason question <id> blocks nothing, for the write that makes
/// an item wait on it: the question blocks that item now.
pub fn clear_blocks_nothing(db: &Connection, id: Id) -> Result<()> {
    db.execute(
        "UPDATE question SET blocks_nothing = NULL WHERE id = ?1",
        [id],
    )
    .sql()?;
    Ok(())
}

/// Writes record <origin> as question <id>'s origin, for
/// `trail::set_origin`, which checks it.
pub(super) fn write_origin(db: &Connection, id: Id, origin: Id) -> Result<()> {
    db.execute(
        "UPDATE question SET origin = ?2 WHERE id = ?1",
        params![id, origin],
    )
    .sql()?;
    Ok(())
}

/// Every question, open and closed, as the rows hold it. <opened> is every
/// record's stamp.
pub fn read(
    db: &Connection,
    opened: &BTreeMap<Id, Stamp>,
    tags: &BTreeMap<Id, Vec<String>>,
) -> Result<Vec<Question>> {
    let mut placements: BTreeMap<Id, Placement> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT question_id, at, rank, why, by_person FROM question_placement")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })
        .sql()?
    {
        let (id, at, rank, why, by_person) = row.sql()?;
        let at = match (at.as_str(), rank) {
            ("first", Some(rank)) => Position::First { rank },
            ("last", None) => Position::Last,
            _ => fail!(
                "question {id}'s placement holds {at} with rank {rank:?}, which question_placement refuses"
            ),
        };
        placements.insert(id, Placement { at, why, by_person });
    }
    let mut plans: BTreeMap<Id, PlanRef> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT question_id, path, number FROM question_plan")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .sql()?
    {
        let (id, path, number) = row.sql()?;
        plans.insert(id, PlanRef { path, number });
    }
    let mut statement = db
        .prepare(
            "SELECT id, title, details, kind, blocks_nothing, origin FROM question ORDER BY id",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<Id>>(5)?,
            ))
        })
        .sql()?;
    let mut questions = Vec::new();
    for row in rows {
        let (id, title, details, kind, blocks_nothing, origin) = row.sql()?;
        questions.push(Question {
            id,
            title,
            details,
            kind: match kind.as_str() {
                "question" => QuestionKind::Question,
                "your-step" => QuestionKind::YourStep,
                _ => QuestionKind::Held,
            },
            blocks_nothing,
            placed: placements.remove(&id),
            plan: plans.remove(&id),
            origin,
            opened: opened[&id].clone(),
            tags: tags.get(&id).cloned().unwrap_or_default(),
        });
    }
    Ok(questions)
}

/// Every open item that waits on question <question> directly, which an
/// answer must say what to do with.
fn waiting_on(lists: &Lists, question: Id) -> Vec<Id> {
    lists
        .items
        .iter()
        .filter(|i| i.waits.contains(&Wait::Question(question)))
        .map(|i| i.id)
        .collect()
}

/// Is <blocked> exactly the items waiting on <question>, each with one fate?
/// Names the items left out, and the ones named that do not wait.
pub fn check_blocked(lists: &Lists, question: Id, blocked: &BlockedMap) -> Result<()> {
    let waiting = waiting_on(lists, question);
    let left_out: Vec<String> = waiting
        .iter()
        .filter(|id| !blocked.contains_key(id))
        .map(|id| id.to_string())
        .collect();
    if !left_out.is_empty() {
        fail!(
            "{} wait{} on {question} and `blocked` says nothing about {}: give each `proceeds`, `edit` or `drop`",
            left_out.join(", "),
            if left_out.len() == 1 { "s" } else { "" },
            if left_out.len() == 1 { "it" } else { "them" }
        );
    }
    let stray: Vec<String> = blocked
        .keys()
        .filter(|id| !waiting.contains(id))
        .map(|id| id.to_string())
        .collect();
    if !stray.is_empty() {
        fail!(
            "`blocked` names {}, which do not wait on {question}",
            stray.join(", ")
        );
    }
    for (id, fate) in blocked {
        match fate {
            Blocked::Drop(why) if why.trim().is_empty() => {
                fail!("`blocked` drops {id} with no reason")
            }
            Blocked::Edit(edit) if edit.is_empty() => {
                fail!("`blocked` edits {id} but names no field; `proceeds` leaves it as it is")
            }
            _ => {}
        }
    }
    Ok(())
}

/// Does what <blocked> says to each item that waits on <question>, after
/// `check_blocked` accepts it.
pub fn apply_blocked(
    db: &Connection,
    question: Id,
    blocked: &BlockedMap,
    stamp: &Stamp,
) -> Result<()> {
    check_blocked(&store::read(db)?, question, blocked)?;
    for (id, fate) in blocked {
        match fate {
            Blocked::Proceeds => {}
            Blocked::Edit(edit) => edit.apply(db, *id)?,
            Blocked::Drop(why) => close(db, *id, How::Dropped(why.clone()), stamp)?,
        }
    }
    Ok(())
}

/// Rebuilds a store's `question` table with the origin column and its index
/// from question.sql, and copies the old rows into it with no origin. The
/// statements come from question.sql, so the table equals a fresh database's.
/// migrate.rs runs this with the foreign keys off, and SQLite renames the old
/// table under `legacy_alter_table`, so the tables that point at `question`
/// keep naming it.
pub fn move_to_origins(db: &Connection) -> Result<()> {
    const SQL: &str = include_str!("question.sql");
    crate::db::remake_table(
        db,
        "question",
        crate::db::statement(SQL, "CREATE TABLE question (", "STRICT;"),
        |db| {
            db.execute_batch(
                "INSERT INTO question (id, record_kind, title, details, kind, blocks_nothing)
    SELECT id, record_kind, title, details, kind, blocks_nothing FROM question_old;",
            )
            .sql()
        },
        &[crate::db::statement(
            SQL,
            "CREATE INDEX question_origin",
            ";",
        )],
    )
}

/// Makes `question` again as question.sql writes it and copies the old rows
/// into it. The copy clears the reason the question blocks nothing on
/// each open question that an open item waits on, because the item's wait already says
/// what the question blocks. The statements come from question.sql, so the table equals a fresh
/// database's. migrate.rs runs this with the foreign keys off, and SQLite
/// renames the old table under `legacy_alter_table`, so the tables that
/// point at `question` keep naming it.
pub fn move_to_blocks_from_waits(db: &Connection) -> Result<()> {
    const SQL: &str = include_str!("question.sql");
    crate::db::remake_table(
        db,
        "question",
        crate::db::statement(SQL, "CREATE TABLE question (", "STRICT;"),
        |db| {
            db.execute_batch(
                "INSERT INTO question (id, record_kind, title, details, kind, blocks_nothing, origin)
    SELECT q.id, q.record_kind, q.title, q.details, q.kind,
        CASE WHEN NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = q.id)
            AND EXISTS (SELECT 1 FROM item_wait_question w WHERE w.on_question = q.id
                AND NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = w.item_id))
        THEN NULL ELSE q.blocks_nothing END,
        q.origin
    FROM question_old q;",
            )
            .sql()
        },
        &[crate::db::statement(
            SQL,
            "CREATE INDEX question_origin",
            ";",
        )],
    )
}

/// Makes `question_placement` again as question.sql writes it, with `rank`,
/// and copies the old rows into it. The stack showed the questions placed
/// first newest asked on top, so each gets a rank by when it was asked, the
/// newest the highest, and keeps its place. The statement comes from
/// question.sql, so the table equals a fresh database's. migrate.rs runs
/// this with the foreign keys off.
pub fn move_to_ranks(db: &Connection) -> Result<()> {
    const SQL: &str = include_str!("question.sql");
    crate::db::remake_table(
        db,
        "question_placement",
        crate::db::statement(SQL, "CREATE TABLE question_placement (", "STRICT;"),
        |db| {
            db.execute_batch(
                "INSERT INTO question_placement (question_id, at, rank, why, by_person)
    SELECT p.question_id, p.at,
        CASE WHEN p.at = 'first' THEN ROW_NUMBER() OVER (PARTITION BY p.at ORDER BY r.opened_at, p.question_id) END,
        p.why, p.by_person
    FROM question_placement_old p JOIN record r ON r.id = p.question_id;",
            )
            .sql()
        },
        &[],
    )
}

/// The tables question.rs owns.
#[cfg(test)]
const TABLES: [&str; 3] = ["question", "question_placement", "question_plan"];

/// Writes a row of one of question.rs's tables as given, the constraints
/// still holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "question_tests.rs"]
mod tests;
