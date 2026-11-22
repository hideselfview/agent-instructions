//! What `builds health` says of an idle claimed worktree's review. It uses
//! the same verdict as the review check in `builds land`.

use crate::health::Advice;
use crate::lists::Lists;
use crate::lists::running::running_in;
use crate::review::landing::{self, Verdict};

/// What the review says of an idle branch's commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Landing {
    /// The review does not take them yet: the clauses of `landing::blockers`.
    Blocked(Vec<String>),
    /// The review permits the commits. The command checks the other
    /// requirements before landing.
    ReviewClear(String),
}

impl Landing {
    /// What `builds health` says of the branch.
    pub fn advice(&self) -> Advice {
        match self {
            Landing::Blocked(blockers) => Advice::said(format!(
                "its branch cannot land yet: {}",
                blockers.join("; ")
            )),
            Landing::ReviewClear(command) => Advice::run(
                "the review check allows these commits. To check app coverage and land, run"
                    .to_string(),
                command.clone(),
            ),
        }
    }
}

/// The review status of the worktree <top> from <verdict>: the review
/// blockers, or the landing command with the items from <lists> running
/// there. <ahead> holds the full hashes of the commits origin/main lacks,
/// oldest first.
pub fn landing(
    verdict: &Verdict,
    lists: &Lists,
    top: &std::path::Path,
    ahead: &[String],
) -> Landing {
    let blockers = landing::blockers(verdict);
    if !blockers.is_empty() {
        return Landing::Blocked(blockers);
    }
    let mut command = format!("builds land {}", top.display());
    for item in running_in(lists, &top.display().to_string()) {
        command.push_str(&format!(" --done {}", item.id.0));
    }
    for sha in ahead {
        command.push(' ');
        command.push_str(sha);
    }
    Landing::ReviewClear(command)
}
