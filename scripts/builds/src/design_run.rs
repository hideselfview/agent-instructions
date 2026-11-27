//! The design commands that run for a worktree: a `builds design record` or
//! `amend`, which draws the boards it records, and a `builds shots draws`,
//! which compares boards at two commits. Health shows an agent that waits on
//! one as waiting on it.

use serde::{Deserialize, Serialize};

use crate::command::{Command, Parsed, design, shots};

/// Which design command runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesignCommand {
    /// `builds design record` or `builds design amend`.
    Record,
    /// `builds shots draws`.
    Comparison,
}

impl DesignCommand {
    /// The design command that the command line <argv> of a running builds
    /// process is; None for any other command.
    pub fn of(argv: &[String]) -> Option<Self> {
        Self::of_words(crate::ack::agent::without_flag(argv).get(1..)?)
    }

    /// `of` for the words after the program.
    pub fn of_words(words: &[&str]) -> Option<Self> {
        match Command::of(words)? {
            Parsed {
                command: Command::Design(design::Record | design::Amend),
                ..
            } => Some(Self::Record),
            Parsed {
                command: Command::Shots(shots::Checker),
                rest: ["draws", ..],
                ..
            } => Some(Self::Comparison),
            _ => None,
        }
    }

    /// What health calls it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Record => "design record",
            Self::Comparison => "board comparison",
        }
    }
}

/// A design command that someone other than builds started in a worktree. One
/// that a builds process started is a step of that command (a record draws the
/// boards, and the dashboard and a landing draw them to judge design
/// coverage).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignRun {
    pub pid: i32,
    /// Index into the places it was placed among, from its working directory.
    #[serde(skip)]
    pub place: usize,
    pub command: DesignCommand,
}

#[cfg(test)]
#[path = "design_run_tests.rs"]
mod tests;
