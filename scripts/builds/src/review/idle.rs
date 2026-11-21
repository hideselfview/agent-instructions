//! What `builds health` says of an idle claimed worktree's branch: whether
//! its review lets the commits land, as the verdict that `landing::judge`
//! gives from the same records and branch `builds land` reads.

use crate::health::Advice;
use crate::lists::Lists;
use crate::lists::running::running_in;
use crate::review::landing::{self, Verdict};

/// What the review says of an idle branch's commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Landing {
    /// The review does not take them yet: the clauses of `landing::blockers`.
    Blocked(Vec<String>),
    /// Nothing blocks landing: the command that lands them.
    Free(String),
}

impl Landing {
    /// What `builds health` says of the branch.
    pub fn advice(&self) -> Advice {
        match self {
            Landing::Blocked(blockers) => Advice::said(format!(
                "its branch cannot land yet: {}",
                blockers.join("; ")
            )),
            Landing::Free(command) => {
                Advice::run("nothing blocks landing: run".to_string(), command.clone())
            }
        }
    }
}

/// What blocks the commits of the worktree <top>, whose project's lists are
/// <lists>, from landing, as <verdict> says of them: the clauses of
/// `landing::blockers`, or the command that lands them, which names the items
/// running there. <ahead> holds the full hashes of the commits origin/main
/// lacks, oldest first.
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
    Landing::Free(command)
}
