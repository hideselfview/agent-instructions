//! The lists: a project's decision stack, work queue, design changes and
//! decision log, kept in one store that every command writes through and
//! refuses a step missing what it needs (plans/lists-tool.md).
//!
//! This file is the data model. `store` holds the file and the write path,
//! `check` the whole-state check, `derive` what is computed and never stored,
//! `view` the JSON the page reads, `draft` the item draft and the record
//! operations the nouns share, `decision` the decision log's commands, `args`
//! the flag parsing every command uses.

pub mod args;
pub mod check;
mod command;
pub mod day;
mod decision;
pub mod derive;
mod design;
mod design_command;
mod design_paths;
pub mod draft;
#[cfg(test)]
pub mod fixtures;
mod show;
pub mod store;
pub mod view;

pub use command::cmd_lists;
pub use decision::cmd_decision;
pub use design_command::cmd_design;

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::Result;

/// A record's id, from one counter per project; printed `#N`. Commands take
/// it as a bare number, since an unquoted `#12` is a comment in the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub u32);

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

/// One line of the write log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogLine {
    pub version: u64,
    pub at: i64,
    pub by: String,
    /// The command's words.
    pub words: String,
}

/// The whole state of one project's lists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lists {
    pub project: String,
    /// Repo directory names, as `Checkout::repo` gives them.
    pub repos: Vec<String>,
    /// Per repo, the globs of its design places: a path a commit changes
    /// that matches one is a design place.
    pub design_places: BTreeMap<String, Vec<String>>,
    /// The next id to hand out.
    pub next_id: Id,
    /// Counts the writes; the log's last line carries it.
    pub version: u64,
    /// In the order the sidebar shows them.
    pub areas: Vec<String>,
    pub questions: Vec<Question>,
    pub items: Vec<Item>,
    pub designs: Vec<DesignChange>,
    pub decisions: Vec<Decision>,
    pub closed: Vec<Closed>,
    pub log: Vec<LogLine>,
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

/// Whose work an item is. The coordinator dispatches `Coordinator` items;
/// `DesignSession` items are shown apart and never listed as dispatchable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Audience {
    Coordinator,
    DesignSession,
}

/// What an item waits on. A condition is met only when removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wait {
    Item(Id),
    Question(Id),
    Design(Id),
    Condition(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    First,
    Last,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Priority {
    pub level: Level,
    pub why: String,
}

/// An item an agent is working on. The agent is the worktree's claim, so the
/// claim and this marker can't disagree; the repo is the worktree's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Running {
    /// The absolute path of a side worktree of a project repo.
    pub worktree: String,
    pub branch: String,
    pub brief: String,
    pub since: Stamp,
}

/// Part of an item already on main.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandedPart {
    pub repo: String,
    pub commits: Vec<String>,
    pub at: i64,
}

/// A progress line on an item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub at: i64,
    pub by: String,
    pub text: String,
}

/// An entry of the work queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: Id,
    /// Five to seven words.
    pub title: String,
    pub area: String,
    pub details: String,
    #[serde(rename = "for")]
    pub r#for: Audience,
    pub waits: Vec<Wait>,
    pub priority: Option<Priority>,
    /// The implementation brief: path, then `#section` when it names one.
    pub brief: Option<String>,
    pub running: Option<Running>,
    pub landed: Vec<LandedPart>,
    /// Newest last.
    pub notes: Vec<Note>,
    /// The decision or design change that made it.
    pub from: Option<Id>,
    pub opened: Stamp,
}

/// How a design place was resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    /// The item that does the app work for it.
    Item(Id),
    /// How it was checked that no app work is needed.
    Matches(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignLanding {
    pub commits: Vec<String>,
    pub at: i64,
}

/// A change to the design files, recorded before it lands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignChange {
    pub id: Id,
    pub title: String,
    pub details: String,
    pub repo: String,
    pub places: BTreeMap<String, Resolution>,
    /// Why shared design files changed with no board or component.
    pub no_app_place: Option<String>,
    pub opened: Stamp,
    pub landed: Vec<DesignLanding>,
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

/// The repo and checkout name of a side worktree path,
/// `<parent>/<repo>-worktrees/side-N`, by its shape alone. The checks that
/// run on every load can't ask git; whether the folder is a live worktree is
/// asked where a command takes the item.
pub fn side_worktree_parts(path: &str) -> Option<(String, String)> {
    let path = std::path::Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let checkout = path.file_name()?.to_str()?;
    let digits = checkout.strip_prefix("side-")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let folder = path.parent()?.file_name()?.to_str()?;
    let repo = folder.strip_suffix("-worktrees")?;
    if repo.is_empty() {
        return None;
    }
    Some((repo.to_string(), checkout.to_string()))
}

impl Lists {
    /// A new project's lists: nothing yet, version 0.
    pub fn empty(project: &str, repos: Vec<String>, design_places: BTreeMap<String, Vec<String>>) -> Lists {
        Lists {
            project: project.to_string(),
            repos,
            design_places,
            next_id: Id(1),
            version: 0,
            areas: Vec::new(),
            questions: Vec::new(),
            items: Vec::new(),
            designs: Vec::new(),
            decisions: Vec::new(),
            closed: Vec::new(),
            log: Vec::new(),
        }
    }

    /// Hands out the next id.
    pub fn alloc_id(&mut self) -> Id {
        let id = self.next_id;
        self.next_id = Id(id.0 + 1);
        id
    }

    pub fn question(&self, id: Id) -> Option<&Question> {
        self.questions.iter().find(|q| q.id == id)
    }

    pub fn item(&self, id: Id) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: Id) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    pub fn design(&self, id: Id) -> Option<&DesignChange> {
        self.designs.iter().find(|d| d.id == id)
    }

    pub fn design_mut(&mut self, id: Id) -> Option<&mut DesignChange> {
        self.designs.iter_mut().find(|d| d.id == id)
    }

    pub fn decision(&self, id: Id) -> Option<&Decision> {
        self.decisions.iter().find(|d| d.id == id)
    }

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

    /// The open items whose waits name <wait>.
    pub fn open_items_waiting_on<'a>(&'a self, wait: &'a Wait) -> impl Iterator<Item = &'a Item> + 'a {
        self.items.iter().filter(move |item| item.waits.contains(wait))
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
