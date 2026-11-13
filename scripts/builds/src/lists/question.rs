//! The decision stack's questions: what a question is, where it sits, and
//! every write to one, with the fates an answer gives the items that waited
//! on it. `question_command` is the commands around these operations.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::Result;
use crate::lists::item::ItemEdit;
use crate::lists::record::close;
use crate::lists::{How, Id, Lists, Stamp, Wait};

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

/// Adds <question> to the stack.
pub fn add(lists: &mut Lists, question: Question) {
    lists.questions.push(question);
}

/// Sets, or with None clears, open question <id>'s placement.
pub fn place(lists: &mut Lists, id: Id, placed: Option<Placement>) -> Result<()> {
    let Some(question) = lists.questions.iter_mut().find(|q| q.id == id) else { fail!("{id} is not an open question") };
    question.placed = placed;
    Ok(())
}

/// Changes open question <id>: the title and details when given, and its
/// blocks-nothing why: None leaves it, Some(None) clears it (the question
/// blocks something), Some(Some(why)) sets it.
pub fn edit(
    lists: &mut Lists,
    id: Id,
    title: Option<String>,
    details: Option<String>,
    blocks_nothing: Option<Option<String>>,
) -> Result<()> {
    let Some(question) = lists.questions.iter_mut().find(|q| q.id == id) else { fail!("{id} is not an open question") };
    if let Some(title) = title {
        question.title = title;
    }
    if let Some(details) = details {
        question.details = details;
    }
    if let Some(why) = blocks_nothing {
        question.blocks_nothing = why;
    }
    Ok(())
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
pub fn apply_blocked(lists: &mut Lists, question: Id, blocked: &BlockedMap, stamp: &Stamp) -> Result<()> {
    check_blocked(lists, question, blocked)?;
    for (id, fate) in blocked {
        match fate {
            Blocked::Proceeds => {}
            Blocked::Edit(edit) => edit.apply(lists, *id)?,
            Blocked::Drop(why) => close(lists, *id, How::Dropped(why.clone()), stamp)?,
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "question_tests.rs"]
mod tests;
