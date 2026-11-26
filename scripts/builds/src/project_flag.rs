//! `--project P`, which every command takes. Before a command runs, the
//! acknowledgement check (ack/mod.rs) reads the agent's acknowledgement from
//! the store of the project the agent works in: the one the claim of the
//! worktree it runs in names, else --project's
//! (`lists::store::project_of`). In a repo two projects hold, a command from a
//! worktree whose claim names none therefore needs the flag, whatever the
//! command does. A command that has no other use for a project does not
//! read the flag: `split` takes it out of the arguments here.

use crate::fail;
use crate::fail::Result;
use crate::lists::store::Store;
use crate::places::Places;

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

/// <args> of `builds <command>`: without `--project` for a command that does
/// not read it, and as they are for one that does. Only builds' own words
/// hold the flag: the words of a command that builds runs stay as they are
/// (`own_words`).
pub fn split(command: &str, args: &[String]) -> Result<Split> {
    let (own, passed) = args.split_at(own_words(command, args));
    if reads_project(command, args) {
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
    let usage = format!("usage: builds {command} ... [{PROJECT} P]");
    let (project, mut kept) = take(own, &usage)?;
    kept.extend_from_slice(passed);
    let flag = project.map_or(Flag::Absent, |project| Flag::Unread(project.to_string()));
    Ok(Split { flag, args: kept })
}

/// Fails when no project is named <project>, so that a command that does not
/// read the flag refuses one that names nothing.
pub fn require_exists(places: &Places, project: &str) -> Result<()> {
    if !Store::named(places, project)?.database().is_file() {
        fail!("there are no lists for project {project}");
    }
    Ok(())
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
fn own_words(command: &str, args: &[String]) -> usize {
    if command == "lock" {
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

/// Does `builds <command> <args>` read `--project` itself? Every word of
/// `cargo` and `lake` is the tool's, so builds takes none out.
fn reads_project(command: &str, args: &[String]) -> bool {
    let word = |at: usize| args.get(at).map(String::as_str);
    match command {
        "ack" | "agent" | "area" | "brief" | "cargo" | "claim" | "decision" | "design" | "item"
        | "judge" | "lake" | "land" | "lists" | "new" | "next" | "question" | "review"
        | "rules-on-edit" | "waits" => true,
        "app-build" => {
            word(0) != Some(crate::app_landing::FLAG)
                && word(0) != Some(crate::handed_runs::FLAG)
                && word(1) != Some("--list")
        }
        "checks" => word(0) == Some("stats"),
        "shots" => word(0) == Some("picture"),
        _ => false,
    }
}

#[cfg(test)]
#[path = "project_flag_tests.rs"]
mod tests;
