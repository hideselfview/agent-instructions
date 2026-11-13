//! The lists: a project's decision stack, work queue, design changes and
//! decision log, kept in one store that every command writes through and
//! refuses a step missing what it needs (plans/lists-tool.md).
//!
//! This file is the data model. `store` holds the file and the write path,
//! `check` the whole-state check, `derive` what is computed and never stored,
//! `view` the JSON the page reads, `draft` the item draft and the record
//! operations the nouns share, `decision` the decision log's commands, `args`
//! the flag parsing every command uses.

mod area;
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
pub mod issue;
pub mod item;
mod item_command;
#[cfg(test)]
mod item_fixtures;
mod item_show;
mod landing;
pub mod project;
pub mod question;
pub mod record;
pub mod running;
pub mod show;
pub mod store;
mod take;
pub mod view;

pub use area::cmd_area;
pub use command::cmd_lists;
pub use decision::cmd_decision;
pub use design_command::cmd_design;
pub use item::{Audience, Item, Level, Note, Priority, Wait};
pub use item_command::{cmd_item, cmd_next};
pub use landing::LandedPart;
pub use record::{Closed, How, Id, Kind, Located, Record, Stamp, Standing};
pub use running::{Running, side_worktree_parts};

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
    /// Per repo, the globs of the shared design-system sources (the
    /// component sources, the tokens): a change to one can change any
    /// component's drawing, so a design change touching them lists the
    /// components whose drawing changes.
    pub design_sources: BTreeMap<String, Vec<String>>,
    /// Per repo, the globs of the spec files; in a design change, each
    /// section (by heading) its commits touch is a place.
    pub spec_files: BTreeMap<String, Vec<String>>,
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

/// A board place's link on the published canvas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Board {
    /// The board's link on the canvas.
    pub link: String,
}

/// The theme a board is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn word(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

/// A board on a review page: the board file (a place of the repo's design
/// places, `design/mocks/<name>.dc.html`), what the caption under it says,
/// and the theme it is drawn in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewBoard {
    pub board: String,
    pub caption: String,
    pub theme: Theme,
}

/// One heading of a review page, the words under it, and the boards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSection {
    pub heading: String,
    #[serde(default)]
    pub words: String,
    #[serde(default)]
    pub boards: Vec<ReviewBoard>,
}

/// A change to the design files, recorded when its boards are drawn. It
/// waits on the person's review (`approved`) before it can land.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignChange {
    pub id: Id,
    pub title: String,
    pub details: String,
    pub repo: String,
    /// The worktree (absolute path) the boards are drawn in: the review page
    /// renders every board from it, live, on each request.
    pub worktree: String,
    /// Each design place the change touches, or a component a shared source
    /// changes the drawing of, and how it is resolved.
    pub places: BTreeMap<String, Resolution>,
    /// Each board place's canvas link, by the board's path.
    pub boards: BTreeMap<String, Board>,
    /// How the review page reads: its headings, words and boards, in order.
    pub review: Vec<ReviewSection>,
    /// Why shared design files changed and no drawing did.
    pub no_app_place: Option<String>,
    /// The person's review: set by `design approve`, cleared by an amend,
    /// since the person approved what was drawn before it.
    pub approved: Option<Stamp>,
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

impl Lists {
    pub fn question(&self, id: Id) -> Option<&Question> {
        self.questions.iter().find(|q| q.id == id)
    }

    pub fn design(&self, id: Id) -> Option<&DesignChange> {
        self.designs.iter().find(|d| d.id == id)
    }

    pub fn decision(&self, id: Id) -> Option<&Decision> {
        self.decisions.iter().find(|d| d.id == id)
    }

}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
