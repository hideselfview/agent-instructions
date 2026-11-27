//! The key a worktree's review is recorded under.
//!
//! A branch names its change, so its key stays the same across commits. A
//! review run in a detached worktree has only its HEAD to name it
//! (`default_key`), and the next commit changes that. Reviews, fixes and the
//! landing of one change in a detached worktree would then each name another
//! key, and the first review's landing would stay open for good. `landing_key`
//! gives every command that names the worktree's change the key of the open
//! detached review that read a commit the worktree holds.

use std::path::Path;

use rusqlite::Connection;

use super::record::{self, Change};
use super::{default_key, ended};
use crate::args::Args;
use crate::checkout::Checkout;
use crate::fail::{Fail, Result};
use crate::lists::store::Store;
use crate::side_git::ask;
use crate::tool::Tool;

/// The key of the change <worktree> of <repo> holds: its default key when
/// that has an open landing or no open detached review of the repo read a
/// commit the worktree holds, else the key of that review. Refuses a worktree
/// that holds commits that several open detached reviews read.
pub fn landing_key(db: &Connection, repo: &str, worktree: &Path) -> Result<String> {
    let key = default_key(worktree)?;
    if record::open_landing(db, Change { repo, key: &key })?.is_some() {
        return Ok(key);
    }
    let mut held = Vec::new();
    for landing in record::all_landings(db)? {
        if landing.repo != repo || !landing.is_open() || !landing.key.starts_with("detached:") {
            continue;
        }
        let Some(run) = ended::last_review(db, &landing)? else {
            continue;
        };
        if crate::git::commit_exists(worktree, &run.head)?
            && ask(
                worktree,
                &["merge-base", "--is-ancestor", &run.head, "HEAD"],
            )
            .map_err(Fail::Message)?
            .is_some()
        {
            held.push(landing.key);
        }
    }
    match held.len() {
        0 => Ok(key),
        1 => Ok(held.remove(0)),
        _ => crate::fail!(
            "{} holds commits that several open reviews read: {}. Name the one this change belongs to with --landing KEY.",
            worktree.display(),
            held.join(", ")
        ),
    }
}

/// `builds review key DIR --project P`: prints the key of the change the
/// worktree at DIR holds, which `builds land` names its review by.
pub fn key_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["project"], &[])?;
    let [dir] = args.positional() else {
        crate::fail!(
            "{}",
            crate::command::Command::Review(crate::command::review::Key).usage_text()
        )
    };
    let store = Store::named(&tool.places, args.require("project")?)?;
    let checkout = Checkout::find(dir)?;
    let db = store.open_existing()?;
    crate::output::answer_line(landing_key(&db, &checkout.repo(), &checkout.top)?);
    Ok(())
}

#[cfg(test)]
#[path = "key_tests.rs"]
mod tests;
