//! The item draft every command that makes work takes, the `blocked` map an
//! answer carries, and the record operations the nouns share: adding an
//! item, closing a record, adding a decision with its work. Operations work
//! on the state a write holds under the lock; the write lands whole or not
//! at all, so an operation that fails midway leaves nothing behind.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::Result;
use crate::lists::check::title_fault;
use crate::lists::item::{ItemDraft, ItemEdit, add_item};
use crate::lists::record::close;
use crate::lists::{Decision, How, Id, Kind, Lists, PlanRef, Stamp, Wait};
use crate::text::local_day;

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

/// Where a relative brief path is looked for. Briefs are plans' files, which
/// no commit carries (`plans/` is ignored), so it is the main checkout of the
/// repo the command runs in; an absolute path is itself. A command with a
/// brief and no checkout is refused.
pub fn brief_base(cwd: &Path) -> Result<std::path::PathBuf> {
    match crate::checkout::Checkout::at(cwd)? {
        Some(checkout) => Ok(checkout.main),
        None => fail!(
            "{} is in no git checkout, so a relative brief path has nothing to be relative to; give the brief as an absolute path",
            cwd.display()
        ),
    }
}

/// A brief path, `path` or `path#section`: the file must exist. A relative
/// path is looked for under <base>. This reads the file system, so a command
/// runs it before it takes the lock.
pub fn require_brief(base: &Path, brief: &str) -> Result<()> {
    let path = brief.split('#').next().unwrap_or("");
    if path.is_empty() {
        fail!("brief '{brief}' names no file");
    }
    let file = base.join(path);
    if !file.is_file() {
        fail!("brief {brief} does not exist: no file {}", file.display());
    }
    Ok(())
}

/// <brief> exists, as a command running in <cwd> sees it: an absolute path is
/// itself, a relative one is under the main checkout of <cwd>'s repo. Only a
/// relative brief needs a checkout.
pub fn require_brief_from(cwd: &Path, brief: &str) -> Result<()> {
    if Path::new(brief).is_absolute() {
        return require_brief(Path::new("/"), brief);
    }
    require_brief(&brief_base(cwd)?, brief)
}

/// The briefs of <drafts> exist (see `require_brief_from`).
pub fn require_draft_briefs(cwd: &Path, drafts: &[ItemDraft]) -> Result<()> {
    drafts.iter().filter_map(|draft| draft.brief.as_deref()).try_for_each(|brief| require_brief_from(cwd, brief))
}

impl ItemDraft {
    /// The brief's file exists (see `require_brief`).
    pub fn require_brief(&self, base: &Path) -> Result<()> {
        match &self.brief {
            Some(brief) => require_brief(base, brief),
            None => Ok(()),
        }
    }
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

/// A request's `work` (item drafts) or `no_work` (why there is none): one
/// of them, never both and never neither.
pub fn work_or_why(work: Option<Vec<ItemDraft>>, no_work: Option<String>) -> Result<Vec<ItemDraft>> {
    match (work, no_work) {
        (Some(_), Some(_)) => fail!("give `work` or `no_work`, not both"),
        (None, None) => fail!("give `work` (the items this makes) or `no_work` with the reason there is none"),
        (None, Some(why)) if why.trim().is_empty() => fail!("`no_work` needs its reason"),
        (None, Some(_)) => Ok(Vec::new()),
        (Some(work), None) => Ok(work),
    }
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
#[path = "draft_tests.rs"]
mod tests;
