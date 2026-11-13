//! The decision stack's questions: what a question is, where it sits, and
//! every write to one, with the fates an answer gives the items that waited
//! on it. `question_command` is the commands around these operations.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::item::ItemEdit;
use crate::lists::record::{self, close};
use crate::lists::store;
use crate::lists::{How, Id, Kind, Lists, Stamp, Wait};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    First,
    Last,
}

/// Where a question sits in the stack when not by how much it unblocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub at: Position,
    pub why: String,
    pub by_person: bool,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: Id,
    pub title: String,
    pub details: String,
    pub kind: QuestionKind,
    /// Why it is asked though nothing waits on it.
    pub blocks_nothing: Option<String>,
    pub placed: Option<Placement>,
    pub plan: Option<PlanRef>,
    pub opened: Stamp,
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
    pub placed: Option<Placement>,
    pub plan: Option<PlanRef>,
}

fn kind_word(kind: QuestionKind) -> &'static str {
    match kind {
        QuestionKind::Question => "question",
        QuestionKind::YourStep => "your-step",
        QuestionKind::Held => "held",
    }
}

fn position_word(at: Position) -> &'static str {
    match at {
        Position::First => "first",
        Position::Last => "last",
    }
}

/// Writes the placement of question <id>, replacing one it has.
fn insert_placement(db: &Connection, id: Id, placed: &Placement) -> Result<()> {
    db.execute(
        "INSERT INTO question_placement (question_id, at, why, by_person) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (question_id) DO UPDATE SET at = ?2, why = ?3, by_person = ?4",
        params![id, position_word(placed.at), placed.why, placed.by_person],
    )
    .sql()?;
    Ok(())
}

/// Asks <new> on the stack, opened by <stamp>; returns its id.
pub fn ask(db: &Connection, new: NewQuestion, stamp: &Stamp) -> Result<Id> {
    let id = record::open(db, Kind::Question, stamp)?;
    insert(db, id, &new)?;
    Ok(id)
}

/// The question's own rows, for the record <id> already opened.
fn insert(db: &Connection, id: Id, new: &NewQuestion) -> Result<()> {
    db.execute(
        "INSERT INTO question (id, title, details, kind, blocks_nothing) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, new.title, new.details, kind_word(new.kind), new.blocks_nothing],
    )
    .sql()?;
    if let Some(placed) = &new.placed {
        insert_placement(db, id, placed)?;
    }
    if let Some(plan) = &new.plan {
        db.execute("INSERT INTO question_plan (question_id, path, number) VALUES (?1, ?2, ?3)", params![id, plan.path, plan.number])
            .sql()?;
    }
    Ok(())
}

/// Puts <question> in as it was written elsewhere; its record is open already.
pub fn move_in(db: &Connection, question: &Question) -> Result<()> {
    insert(
        db,
        question.id,
        &NewQuestion {
            title: question.title.clone(),
            details: question.details.clone(),
            kind: question.kind,
            blocks_nothing: question.blocks_nothing.clone(),
            placed: question.placed.clone(),
            plan: question.plan.clone(),
        },
    )
}

/// Question <id> is open, or the refusal.
fn require_open_question(db: &Connection, id: Id) -> Result<()> {
    match record::standing(db, id)? {
        Some((Kind::Question, None)) => Ok(()),
        _ => fail!("{id} is not an open question"),
    }
}

/// Sets, or with None clears, open question <id>'s placement.
pub fn place(db: &Connection, id: Id, placed: Option<Placement>) -> Result<()> {
    require_open_question(db, id)?;
    match placed {
        Some(placed) => insert_placement(db, id, &placed),
        None => {
            db.execute("DELETE FROM question_placement WHERE question_id = ?1", [id]).sql()?;
            Ok(())
        }
    }
}

/// Changes open question <id>: the title and details when given, and its
/// blocks-nothing why: None leaves it, Some(None) clears it (the question
/// blocks something), Some(Some(why)) sets it.
pub fn edit(
    db: &Connection,
    id: Id,
    title: Option<String>,
    details: Option<String>,
    blocks_nothing: Option<Option<String>>,
) -> Result<()> {
    require_open_question(db, id)?;
    db.execute(
        "UPDATE question SET title = COALESCE(?2, title), details = COALESCE(?3, details), \
         blocks_nothing = CASE WHEN ?4 THEN ?5 ELSE blocks_nothing END WHERE id = ?1",
        params![id, title, details, blocks_nothing.is_some(), blocks_nothing.flatten()],
    )
    .sql()?;
    Ok(())
}

/// Every question, open and closed, as the rows hold it. <opened> is every
/// record's stamp.
pub fn read(db: &Connection, opened: &BTreeMap<Id, Stamp>) -> Result<Vec<Question>> {
    let mut placements: BTreeMap<Id, Placement> = BTreeMap::new();
    let mut statement = db.prepare("SELECT question_id, at, why, by_person FROM question_placement").sql()?;
    for row in statement
        .query_map([], |row| Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, bool>(3)?)))
        .sql()?
    {
        let (id, at, why, by_person) = row.sql()?;
        placements.insert(id, Placement { at: if at == "first" { Position::First } else { Position::Last }, why, by_person });
    }
    let mut plans: BTreeMap<Id, PlanRef> = BTreeMap::new();
    let mut statement = db.prepare("SELECT question_id, path, number FROM question_plan").sql()?;
    for row in statement
        .query_map([], |row| Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
        .sql()?
    {
        let (id, path, number) = row.sql()?;
        plans.insert(id, PlanRef { path, number });
    }
    let mut statement = db.prepare("SELECT id, title, details, kind, blocks_nothing FROM question ORDER BY id").sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, Option<String>>(4)?))
        })
        .sql()?;
    let mut questions = Vec::new();
    for row in rows {
        let (id, title, details, kind, blocks_nothing) = row.sql()?;
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
            opened: opened[&id].clone(),
        });
    }
    Ok(questions)
}

/// Every open item that waits on question <question> directly, which an
/// answer must say what to do with.
fn waiting_on(lists: &Lists, question: Id) -> Vec<Id> {
    lists.items.iter().filter(|i| i.waits.contains(&Wait::Question(question))).map(|i| i.id).collect()
}

/// Is <blocked> exactly the items waiting on <question>, each with one fate?
/// Names the items left out, and the ones named that do not wait.
pub fn check_blocked(lists: &Lists, question: Id, blocked: &BlockedMap) -> Result<()> {
    let waiting = waiting_on(lists, question);
    let left_out: Vec<String> = waiting.iter().filter(|id| !blocked.contains_key(id)).map(|id| id.to_string()).collect();
    if !left_out.is_empty() {
        fail!(
            "{} wait{} on {question} and `blocked` says nothing about {}: give each `proceeds`, `edit` or `drop`",
            left_out.join(", "),
            if left_out.len() == 1 { "s" } else { "" },
            if left_out.len() == 1 { "it" } else { "them" }
        );
    }
    let stray: Vec<String> = blocked.keys().filter(|id| !waiting.contains(id)).map(|id| id.to_string()).collect();
    if !stray.is_empty() {
        fail!("`blocked` names {}, which do not wait on {question}", stray.join(", "));
    }
    for (id, fate) in blocked {
        match fate {
            Blocked::Drop(why) if why.trim().is_empty() => fail!("`blocked` drops {id} with no reason"),
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
pub fn apply_blocked(db: &Connection, question: Id, blocked: &BlockedMap, stamp: &Stamp) -> Result<()> {
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

/// The tables question.rs owns.
#[cfg(test)]
const TABLES: [&str; 3] = ["question", "question_placement", "question_plan"];

/// Writes a row of one of question.rs's tables as given, the constraints
/// still holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(db: &Connection, table: &str, columns: &[(&str, rusqlite::types::Value)]) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "question_tests.rs"]
mod tests;
