//! `--project P`, which every command takes. Before a command runs, the
//! acknowledgement check (ack/mod.rs) reads the agent's acknowledgement from
//! the store of the project the agent works in: the one the claim of the
//! worktree it runs in names, else --project's
//! (`lists::store::project_of`). In a repo two projects hold, a command from a
//! worktree whose claim names none therefore needs the flag, whatever the
//! command does. A command that has no other use for a project does not read
//! the flag, and main checks the project it names instead.

use crate::args::Args;
use crate::fail::Result;

/// The flag that names a project.
pub const PROJECT: &str = "--project";

/// The `--project` of a command.
#[derive(Debug, PartialEq, Eq)]
pub enum Flag {
    Absent,
    /// The command reads it, and checks the project it names.
    Read(String),
    /// The command does not read it.
    Unread(String),
}

impl Flag {
    /// The `--project` of <args>.
    pub fn of(args: &Args) -> Result<Flag> {
        Ok(match args.value("project")? {
            None => Flag::Absent,
            Some(project) if args.reads_project() => Flag::Read(project.to_string()),
            Some(project) => Flag::Unread(project.to_string()),
        })
    }

    /// The project the flag names, for the acknowledgement check.
    pub fn project(&self) -> Option<&str> {
        match self {
            Flag::Absent => None,
            Flag::Read(project) | Flag::Unread(project) => Some(project),
        }
    }
}

impl Args {
    /// Whether the command reads `--project` itself (project_flag.rs). Every
    /// word of `cargo` and `lake` is the tool's.
    pub fn reads_project(&self) -> bool {
        use crate::command::Command::*;
        use crate::command::*;
        match self.command() {
            Ack
            | Brief
            | Cargo
            | Claim
            | Discard
            | Lake
            | Land
            | New
            | Next
            | RulesOnEdit
            | Waits
            | Agent(agent::Start | agent::Run | agent::Resume | agent::List | agent::Show)
            | Area(area::Add | area::Rename | area::Move | area::Remove)
            | Decision(
                decision::Add | decision::Edit | decision::Find | decision::Show | decision::List,
            )
            | Design(
                design::Places
                | design::Only
                | design::Record
                | design::Amend
                | design::Approve
                | design::Check
                | design::Landed
                | design::Title
                | design::Drop
                | design::Show
                | design::List
                | design::Render
                | design::TokensCss
                | design::PublishFind
                | design::PublishFiles
                | design::PublishLast
                | design::Listen
                | design::PublishTake
                | design::PublishDone
                | design::PublishFailed,
            )
            | Item(
                item::Add
                | item::Edit
                | item::Note
                | item::Changes
                | item::Wait
                | item::Backlog
                | item::Start
                | item::Take
                | item::CheckBrief
                | item::Handoff
                | item::Stop
                | item::Finished
                | item::Drop
                | item::Place
                | item::CheckLanding
                | item::Landed
                | item::Show
                | item::List
                | item::Times,
            )
            | Judge(judge::Listen)
            | Lists(
                lists::Init | lists::DesignGlobs | lists::AddRepo | lists::Show | lists::ProjectOf,
            )
            | Question(
                question::Ask
                | question::Answer
                | question::Settle
                | question::Done
                | question::Drop
                | question::Place
                | question::Edit
                | question::Show
                | question::List,
            )
            | Review(
                review::Run
                | review::Copy
                | review::Settle
                | review::Wait
                | review::Dispute
                | review::Keep
                | review::Show
                | review::Key
                | review::CheckLanding
                | review::Landed
                | review::Groups
                | review::Replay
                | review::Audit
                | review::Misses
                | review::Rates
                | review::Scores
                | review::CasePropose
                | review::CaseAdd,
            )
            | Checks(checks::Stats)
            | AppBuild(app_build::DropShots)
            | Shots(shots::Picture) => true,
            // `--list` reads only the repo's steps.
            AppBuild(app_build::Build) => !self.switch("list"),
            Help
            | Setup
            | Tool
            | AppBuild(app_build::LandingCoverage | app_build::RecordHanded)
            | Note
            | Release
            | Retire
            | BuildApp
            | Lock
            | Programs
            | Queue
            | Takeover
            | AppClone
            | SccacheTmpdir
            | Prune
            | Du
            | Status
            | Health
            | InstallHook
            | Install
            | Version
            | Shots(shots::Findings | shots::CheckProgram | shots::Checker)
            | Web(web::Picture)
            | AppCheckout(app_checkout::Add | app_checkout::Remove | app_checkout::Reset)
            | Host(
                host::Add
                | host::Setup
                | host::SetupHere
                | host::SpeedHere
                | host::Band
                | host::List
                | host::Cache
                | host::Remove
                | host::ClearXcode,
            )
            | Remote(remote::Run | remote::Step)
            | Units(units::Prune | units::GitCheckouts | units::AfterBuild)
            | Dashboard(dashboard::Stage | dashboard::Serve)
            | Checks(checks::Run | checks::Whole | checks::List | checks::Tests) => false,
        }
    }
}

#[cfg(test)]
#[path = "project_flag_tests.rs"]
mod tests;
