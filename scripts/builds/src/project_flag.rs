//! `--project P`, which every command takes. Before a command runs, the
//! acknowledgement check (ack/mod.rs) reads the agent's acknowledgement from
//! the store of the project the agent works in: the one the claim of the
//! worktree it runs in names, else --project's
//! (`lists::store::project_of`). In a repo two projects hold, a command from a
//! worktree whose claim names none therefore needs the flag, whatever the
//! command does. A command that has no other use for a project does not
//! read the flag: `split` takes it out of the arguments here.

use crate::command::{Command, Parsed};
use crate::fail;
use crate::fail::Result;

/// The flag that names a project.
pub const PROJECT: &str = "--project";

/// A command's arguments as `split` reads them.
#[derive(Debug, PartialEq, Eq)]
pub struct Split {
    pub flag: Flag,
    /// The arguments the command reads.
    pub args: Vec<String>,
}

/// The `--project` of a command.
#[derive(Debug, PartialEq, Eq)]
pub enum Flag {
    Absent,
    /// The command reads it, so its arguments hold it, and the command checks
    /// the project it names.
    Read(String),
    /// The command does not read it, so its arguments lack it.
    Unread(String),
}

impl Flag {
    /// The project the flag names, for the acknowledgement check.
    pub fn project(&self) -> Option<&str> {
        match self {
            Flag::Absent => None,
            Flag::Read(project) | Flag::Unread(project) => Some(project),
        }
    }
}

/// The words after the command of <parsed>: without `--project` for a
/// command that does not read it (`Parsed::reads_project`), and as they are
/// for one that does. Only builds' own words hold the flag: the words of a
/// command that builds runs stay as they are (`own_words`).
pub fn split(parsed: &Parsed<'_, String>) -> Result<Split> {
    let args = parsed.rest;
    let (own, passed) = args.split_at(own_words(parsed.command, args));
    if parsed.reads_project() {
        let flag = own
            .iter()
            .position(|arg| arg == PROJECT)
            .and_then(|at| own.get(at + 1))
            .map_or(Flag::Absent, |project| Flag::Read(project.clone()));
        return Ok(Split {
            flag,
            args: args.to_vec(),
        });
    }
    let usage = format!("usage: builds {} ... [{PROJECT} P]", parsed.named.join(" "));
    let (project, mut kept) = take(own, &usage)?;
    kept.extend_from_slice(passed);
    let flag = project.map_or(Flag::Absent, |project| Flag::Unread(project.to_string()));
    Ok(Split { flag, args: kept })
}

/// The project that <args>' `--project` names, and <args> without it. A
/// refusal ends with the command's <usage>.
pub fn take<'a>(args: &'a [String], usage: &str) -> Result<(Option<&'a str>, Vec<String>)> {
    let mut rest = args.to_vec();
    let Some(at) = args.iter().position(|arg| arg == PROJECT) else {
        return Ok((None, rest));
    };
    let value = args.get(at + 1).filter(|value| !value.starts_with('-'));
    let Some(value) = value else {
        fail!("{PROJECT} needs a project's name\n{usage}");
    };
    rest.drain(at..at + 2);
    if rest.iter().any(|arg| arg == PROJECT) {
        fail!("{PROJECT} is given twice\n{usage}");
    }
    Ok((Some(value), rest))
}

/// How many of <args> are builds' own words: those before the command that
/// `builds lock` runs, which follows the repo and the lock's flags, and those
/// before `--` for every other command.
fn own_words(command: Command, args: &[String]) -> usize {
    if command == Command::Lock {
        let mut at = 1;
        while args.get(at).is_some_and(|arg| arg.starts_with("--")) {
            at += 2;
        }
        return at.min(args.len());
    }
    args.iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len())
}

impl<S: AsRef<str>> crate::command::Parsed<'_, S> {
    /// Whether the command reads `--project` itself (project_flag.rs). Every
    /// word of `cargo` and `lake` is the tool's.
    pub fn reads_project(&self) -> bool {
        use crate::command::Command::*;
        use crate::command::*;
        let word = |at: usize| self.rest.get(at).map(AsRef::as_ref);
        match self.command {
            Ack
            | Brief
            | Cargo
            | Claim
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
            | Shots(shots::Picture) => true,
            AppBuild => {
                word(0) != Some(crate::app_landing::FLAG)
                    && word(0) != Some(crate::handed_runs::FLAG)
                    && word(1) != Some("--list")
            }
            Help
            | Setup
            | Tool
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
