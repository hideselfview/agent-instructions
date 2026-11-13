//! The lists: a project's decision stack, work queue, design changes and
//! decision log, kept in one store that every command writes through and
//! refuses a step missing what it needs (plans/lists-tool.md).
//!
//! This file is the whole state and the owners' types re-exported.
//! Each owner holds its types and every write to them: `record` (ids, stamps,
//! closing), `project`, `area`, `item` with `running` and `landing`,
//! `question`, `decision`, `design`. Their commands are `command`,
//! `item_command`, `question_command`, `decision_command` and
//! `design_command`. `store` holds the file and the write path, `check` the
//! whole-state check, `derive` what is computed and never stored, `view` the
//! JSON the page reads, `draft` what a command that makes work checks before
//! the lock, `args` the flag parsing every command uses.

mod area;
pub mod args;
pub mod check;
mod command;
pub mod day;
mod decision;
mod decision_command;
pub mod derive;
pub mod design;
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
pub mod question_command;
pub mod record;
pub mod running;
pub mod show;
pub mod store;
mod take;
pub mod view;

pub use area::cmd_area;
pub use command::cmd_lists;
pub use decision::Decision;
pub use decision_command::cmd_decision;
pub use design::{DesignChange, DesignLanding, Resolution, ReviewBoard, Theme};
pub use design_command::cmd_design;
pub use item::{Audience, Item, Level, Note, Priority, Wait};
pub use item_command::{cmd_item, cmd_next};
pub use landing::LandedPart;
pub use question::{Placement, PlanRef, Position, Question, QuestionKind};
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

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
