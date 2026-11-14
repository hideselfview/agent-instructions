//! The decision log's entries: what a decision is, and the write that adds
//! one with the work it makes or edits one. `decision_command` is the
//! commands around these operations; what replaces a decision is derived
//! from the decisions that name it, never stored.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::check::title_fault;
use crate::lists::item::{ItemDraft, Origin, add_item};
use crate::lists::record;
use crate::lists::{How, Id, Item, Kind, Lists, PlanRef, Question, Stamp};
use crate::text::local_day;

impl Lists {
    pub fn decision(&self, id: Id) -> Option<&Decision> {
        self.decisions.iter().find(|d| d.id == id)
    }
}

/// An entry of the log. Never closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub id: Id,
    /// YYYY-MM-DD.
    pub date: String,
    pub title: String,
    /// `person`, `coordinator`, `design-session`, ...
    pub by: String,
    pub body: String,
    /// The question it answered.
    pub question: Option<Id>,
    /// The items it created.
    pub work: Vec<Id>,
    /// The earlier decision it revises.
    pub replaces: Option<Id>,
    pub plan: Option<PlanRef>,
    pub recorded: Stamp,
}
/// A decision's own fields; its date and recorder come from the write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewDecision {
    pub title: String,
    /// Who decided: `person`, `coordinator`, `design-session`, ...
    pub by: String,
    pub body: String,
    pub question: Option<Id>,
    pub replaces: Option<Id>,
    pub plan: Option<PlanRef>,
}

/// What `add_decision` made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub decision: Id,
    pub items: Vec<Id>,
}

/// Adds the decision <new> with the items of <work>, each made by it; the
/// decision's date is the day of <stamp>. A decision that answers a question
/// has that question's plan, so only one that answers none stores its own.
pub fn add_decision(
    db: &Connection,
    new: &NewDecision,
    work: &[ItemDraft],
    stamp: &Stamp,
) -> Result<Added> {
    if let Some(fault) = title_fault(&new.title) {
        fail!("{fault}");
    }
    if new.by.trim().is_empty() {
        fail!("a decision names who decided (`by`)");
    }
    if new.body.trim().is_empty() {
        fail!("the decision's body is empty; say what was decided and why");
    }
    if let Some(question) = new.question
        && !matches!(record::standing(db, question)?, Some((Kind::Question, _)))
    {
        fail!("the decision answers {question}, which is not a question");
    }
    if let Some(replaces) = new.replaces
        && !matches!(record::standing(db, replaces)?, Some((Kind::Decision, _)))
    {
        fail!("the decision replaces {replaces}, which is not a decision");
    }
    let decision = record::open(db, Kind::Decision, stamp)?;
    let mut items = Vec::new();
    for draft in work {
        items.push(add_item(
            db,
            draft,
            Some(Origin::Decision(decision)),
            stamp,
        )?);
    }
    insert(
        db,
        decision,
        &local_day(stamp.at),
        new.title.as_str(),
        new.by.as_str(),
        new.body.as_str(),
        new.replaces,
    )?;
    if let (None, Some(plan)) = (new.question, &new.plan) {
        insert_plan(db, decision, plan)?;
    }
    Ok(Added { decision, items })
}

fn insert(
    db: &Connection,
    id: Id,
    date: &str,
    title: &str,
    by: &str,
    body: &str,
    replaces: Option<Id>,
) -> Result<()> {
    db.execute(
        "INSERT INTO decision (id, date, title, decided_by, body, replaces) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, date, title, by, body, replaces],
    )
    .sql()?;
    Ok(())
}

fn insert_plan(db: &Connection, id: Id, plan: &PlanRef) -> Result<()> {
    db.execute(
        "INSERT INTO decision_plan (decision_id, path, number) VALUES (?1, ?2, ?3)",
        params![id, plan.path, plan.number],
    )
    .sql()?;
    Ok(())
}

/// Puts <decision> in as it was written elsewhere; its record is open
/// already. What it answered and what it made are the closing and the items
/// that name it, put in by theirs; an answering decision's plan is its
/// question's.
pub fn move_in(db: &Connection, decision: &Decision) -> Result<()> {
    insert(
        db,
        decision.id,
        &decision.date,
        &decision.title,
        &decision.by,
        &decision.body,
        decision.replaces,
    )?;
    if let (None, Some(plan)) = (decision.question, &decision.plan) {
        insert_plan(db, decision.id, plan)?;
    }
    Ok(())
}

/// Changes decision <id>'s title and body, those given.
pub fn edit(db: &Connection, id: Id, title: Option<String>, body: Option<String>) -> Result<()> {
    if !matches!(record::standing(db, id)?, Some((Kind::Decision, _))) {
        fail!("{id} is not a decision");
    }
    db.execute(
        "UPDATE decision SET title = COALESCE(?2, title), body = COALESCE(?3, body) WHERE id = ?1",
        params![id, title, body],
    )
    .sql()?;
    Ok(())
}

/// Every decision as the rows hold it. What it answered, what it made and
/// an answering decision's plan are read from the closings, the items and
/// the questions.
pub fn read(
    db: &Connection,
    opened: &BTreeMap<Id, Stamp>,
    closings: &BTreeMap<Id, (How, Stamp)>,
    items: &[Item],
    questions: &[Question],
) -> Result<Vec<Decision>> {
    let mut answered: BTreeMap<Id, Id> = BTreeMap::new();
    for (question, (how, _)) in closings {
        if let How::Answered(decision) = how {
            answered.insert(*decision, *question);
        }
    }
    let mut plans: BTreeMap<Id, PlanRef> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT decision_id, path, number FROM decision_plan")
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
        .prepare("SELECT id, date, title, decided_by, body, replaces FROM decision ORDER BY id")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<Id>>(5)?,
            ))
        })
        .sql()?;
    let mut decisions = Vec::new();
    for row in rows {
        let (id, date, title, by, body, replaces) = row.sql()?;
        let question = answered.get(&id).copied();
        let plan = match question {
            Some(question) => questions
                .iter()
                .find(|q| q.id == question)
                .and_then(|q| q.plan.clone()),
            None => plans.remove(&id),
        };
        decisions.push(Decision {
            id,
            date,
            title,
            by,
            body,
            question,
            work: items
                .iter()
                .filter(|item| item.from == Some(id))
                .map(|item| item.id)
                .collect(),
            replaces,
            plan,
            recorded: opened[&id].clone(),
        });
    }
    Ok(decisions)
}

/// The tables decision.rs owns.
#[cfg(test)]
const TABLES: [&str; 2] = ["decision", "decision_plan"];

/// Writes a row of one of decision.rs's tables as given, the constraints
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
#[path = "decision_tests.rs"]
mod tests;
