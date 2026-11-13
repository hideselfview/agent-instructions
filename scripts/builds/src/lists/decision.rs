//! The decision log's entries: what a decision is, and the write that adds
//! one with the work it makes or edits one. `decision_command` is the
//! commands around these operations; what replaces a decision is derived
//! from the decisions that name it, never stored.

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::Result;
use crate::lists::check::title_fault;
use crate::lists::item::{ItemDraft, add_item};
use crate::lists::{Id, Kind, Lists, PlanRef, Stamp};
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
/// decision's date is the day of <stamp>.
pub fn add_decision(lists: &mut Lists, new: &NewDecision, work: &[ItemDraft], stamp: &Stamp) -> Result<Added> {
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
        && lists.locate(question).is_none_or(|found| found.kind != Kind::Question)
    {
        fail!("the decision answers {question}, which is not a question");
    }
    if let Some(replaces) = new.replaces
        && lists.decision(replaces).is_none()
    {
        fail!("the decision replaces {replaces}, which is not a decision");
    }
    let decision = lists.alloc_id();
    let mut items = Vec::new();
    for draft in work {
        items.push(add_item(lists, draft, Some(decision), stamp)?);
    }
    lists.decisions.push(Decision {
        id: decision,
        date: local_day(stamp.at),
        title: new.title.clone(),
        by: new.by.clone(),
        body: new.body.clone(),
        question: new.question,
        work: items.clone(),
        replaces: new.replaces,
        plan: new.plan.clone(),
        recorded: stamp.clone(),
    });
    Ok(Added { decision, items })
}

/// Changes decision <id>'s title and body, those given.
pub fn edit(lists: &mut Lists, id: Id, title: Option<String>, body: Option<String>) -> Result<()> {
    let Some(decision) = lists.decisions.iter_mut().find(|d| d.id == id) else {
        fail!("{id} is not a decision");
    };
    if let Some(title) = title {
        decision.title = title;
    }
    if let Some(body) = body {
        decision.body = body;
    }
    Ok(())
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod tests;
