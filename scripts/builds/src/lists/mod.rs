//! The lists: a project's decision stack, work queue, design changes and
//! decision log, kept in one SQLite database that every command writes through
//! and refuses a step missing what it needs (plans/lists-tool.md).
//!
//! This file is the whole state as one snapshot and the owners' types
//! re-exported. Each owner holds its types, its tables (the `.sql` file beside
//! it) and every write to them: `record` (ids, stamps, closing), `project`,
//! `area`, `item` with `running`, `kept_work`, `landing` and `dropped`,
//! `item_change` (what someone changed in an open item), `item_event` (the moments an item changed state or gained or lost a wait), `rust_release` (the item made for each stable Rust release), `rust_check`
//! (how the last check for one ended), `question`,
//! `decision`,
//! `design`, `tag` (the tags of questions and items), `spec_check` (the
//! checks of briefs against the product spec), `store` (the write log). `trail` reads where each question and item came from. Their commands
//! are `command`,
//! `item_command`, `question_command`, `decision_command` and
//! `design_command`. `store` also holds the database's opening, the write
//! transaction and the snapshot read, `check` the checks the tables cannot
//! hold, `derive` what is computed and never stored, `view` the JSON the page
//! reads, `draft` what a command that makes work checks before the write,
//! `citation` what the `decision #N` citations in a question name, `args` the flag parsing every command uses.

pub mod area;
pub mod args;
pub mod check;
pub mod citation;
mod command;
pub mod day;
pub mod decision;
mod decision_command;
pub mod derive;
pub mod design;
pub mod design_circle_box;
pub(crate) mod design_command;
pub mod design_drawn;
pub use design_command::change_is_design_only;
pub mod design_paths;
pub mod design_reads;
pub mod design_session;
pub mod draft;
pub mod dropped;
#[cfg(test)]
pub mod fixtures;
pub mod held;
pub mod item;
pub mod item_change;
mod item_command;
pub mod item_event;
#[cfg(test)]
pub mod item_fixtures;
pub(crate) mod item_show;
pub mod kept_work;
pub mod landing;
pub mod pasted;
pub mod project;
pub mod project_file;
pub mod publish;
mod publish_command;
pub mod question;
pub mod question_command;
pub mod record;
pub mod running;
pub mod rust_check;
pub mod rust_release;
pub mod show;
pub mod spec_check;
pub mod store;
pub mod tag;
mod take;
pub mod trail;
pub mod view;

pub use area::cmd_area;
pub use command::cmd_lists;
pub use decision::Decision;
pub use decision_command::cmd_decision;
pub use design::{DesignChange, DesignLanding, Resolution, ReviewEntry, Shown, Theme};
pub use design_command::cmd_design;
pub use item::{Audience, Ender, Item, Level, Note, Priority, Wait};
pub use item_command::{cmd_item, cmd_next, start_items};
pub use landing::LandedPart;
pub use question::{Effect, Placement, PlanRef, Position, Question, QuestionKind};
pub use record::{Closed, How, Id, Kind, Located, Record, Stamp, Standing};
pub use running::{Ended, Running, Span, SpanKind, side_worktree_parts};

use std::collections::BTreeMap;

use serde::Serialize;

/// The whole state of one project's lists.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lists {
    pub project: String,
    /// Does the project publish its boards as a claude.ai canvas? The
    /// dashboard's Publish to claude.ai form then takes its designs.
    pub canvas: bool,
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
    /// The last write's version.
    pub version: u64,
    /// When the store began to record the spans of runs, or None when the
    /// store has recorded them since builds made it.
    pub spans_from: Option<i64>,
    /// In the order the sidebar shows them.
    pub areas: Vec<String>,
    pub questions: Vec<Question>,
    pub items: Vec<Item>,
    pub designs: Vec<DesignChange>,
    pub decisions: Vec<Decision>,
    pub closed: Vec<Closed>,
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
