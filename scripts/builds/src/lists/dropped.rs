//! The work an item held when the person dropped it. Dropping an item that
//! runs or is parked alone in its worktrees throws its work away (`builds
//! item drop`), and drop refuses an item that shares a worktree with another
//! open item (`require_alone`). The drop records each worktree's branch, the
//! commits on it and on HEAD that origin/main lacks and the files that were
//! not committed, and after the store's write commits, it detaches the
//! worktree at origin/main, discards its changes, deletes the branch and
//! removes the worktree's claim. Each git step does nothing to work already
//! thrown away, so the person can run a drop that git failed again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::checkout::{Checkout, git, git_text};
use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::running::{require_side_worktree, running_in};
use crate::lists::{Id, Lists};
use crate::side_git::Commit;

/// What one worktree of a dropped item held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DroppedWork {
    pub repo: String,
    pub worktree: String,
    pub branch: String,
    /// The commits on the branch and on HEAD that origin/main had no copy
    /// of, newest first.
    pub commits: Vec<DroppedCommit>,
    /// The files that were not committed.
    pub files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DroppedCommit {
    pub sha: String,
    pub subject: String,
}

/// The main checkout of the repo of side worktree <worktree>, by the path's
/// shape: <parent>/<repo>.
fn main_of(worktree: &str) -> Result<(String, PathBuf)> {
    let (parent, repo, _) = require_side_worktree(worktree)?;
    let main = Path::new(&parent).join(&repo);
    Ok((repo, main))
}

/// What <worktree> holds of an item's work on <branch>: the commits on the
/// branch and on the worktree's HEAD that origin/main lacks, and the files
/// not committed there. read fails when another worktree checks the branch
/// out, since deleting the branch would take that worktree's work too.
pub fn read(worktree: &str, branch: &str) -> Result<DroppedWork> {
    let (repo, main) = main_of(worktree)?;
    if let Some(other) = crate::checkout::worktree_list(&main)?
        .into_iter()
        .find(|listed| {
            listed.branch.as_deref() == Some(branch) && listed.top != Path::new(worktree)
        })
    {
        fail!(
            "{} checks out {branch}, the branch of the work being dropped; switch it off that branch first",
            other.top.display()
        );
    }
    let live = crate::lists::running::live_worktree(worktree)?;
    let mut found: Vec<Commit> = Vec::new();
    let mut add = |commits: Vec<Commit>| {
        for commit in commits {
            if !found.iter().any(|known| known.full == commit.full) {
                found.push(commit);
            }
        }
    };
    let unread =
        |error: String| fail::Fail::Message(format!("reading the work in {worktree}: {error}"));
    if crate::checkout::branch_exists(&main, branch)? {
        add(
            crate::side_git::waiting_to_land(&main, &format!("refs/heads/{branch}"))
                .map_err(unread)?,
        );
    }
    let mut files = Vec::new();
    if let Some(checkout) = &live {
        add(crate::side_git::waiting_to_land(&checkout.top, "HEAD").map_err(unread)?);
        files = crate::side_git::uncommitted_paths(&checkout.top)
            .map_err(unread)?
            .into_iter()
            .collect();
    }
    Ok(DroppedWork {
        repo,
        worktree: worktree.to_string(),
        branch: branch.to_string(),
        commits: found
            .into_iter()
            .map(|commit| DroppedCommit {
                sha: commit.full,
                subject: commit.subject,
            })
            .collect(),
        files,
    })
}

/// Refuses to drop item <id> of project <project> while another open item of
/// <every> (each project's lists) runs or is parked in one of <worktrees>:
/// the work there is that item's too, and the drop would throw it away and
/// remove the claim of that item's agent. `item stop` ends the item's run
/// and leaves the work to the other items, and a drop of an item that runs
/// nowhere throws nothing away.
pub fn require_alone<'a>(
    every: impl IntoIterator<Item = &'a Lists>,
    project: &str,
    id: Id,
    worktrees: &[&str],
) -> Result<()> {
    let mut others: Vec<String> = Vec::new();
    for lists in every {
        for worktree in worktrees {
            let parked = crate::lists::parked::parked_in(lists, worktree);
            for item in running_in(lists, worktree).into_iter().chain(parked) {
                if lists.project == project && item.id == id {
                    continue;
                }
                let words =
                    item.named((lists.project != project).then_some(lists.project.as_str()));
                if !others.contains(&words) {
                    others.push(words);
                }
            }
        }
    }
    if others.is_empty() {
        return Ok(());
    }
    fail!(
        "{id} shares its worktree with {}, so dropping it would throw away their work and remove their agent's claim. Stop {id} first (`builds item stop {} --why WHY`), which leaves the work to them, then drop it",
        others.join(", "),
        id.0
    );
}

/// Records <works> as what closed item <id> held when the person dropped it.
pub fn record(db: &Connection, id: Id, works: &[DroppedWork]) -> Result<()> {
    for work in works {
        db.execute(
            "INSERT INTO dropped_place (item_id, repo, worktree, branch) VALUES (?1, ?2, ?3, ?4)",
            params![id, work.repo, work.worktree, work.branch],
        )
        .sql()?;
        for (position, commit) in work.commits.iter().enumerate() {
            db.execute(
                "INSERT INTO dropped_commit (item_id, repo, position, sha, subject) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, work.repo, position as i64, commit.sha, commit.subject],
            )
            .sql()?;
        }
        for path in &work.files {
            db.execute(
                "INSERT INTO dropped_file (item_id, repo, path) VALUES (?1, ?2, ?3)",
                params![id, work.repo, path],
            )
            .sql()?;
        }
    }
    Ok(())
}

/// What every dropped item held, by item, in repo order.
pub fn read_all(db: &Connection) -> Result<BTreeMap<Id, Vec<DroppedWork>>> {
    let mut all: BTreeMap<Id, Vec<DroppedWork>> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT item_id, repo, worktree, branch FROM dropped_place ORDER BY item_id, repo")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                DroppedWork {
                    repo: row.get(1)?,
                    worktree: row.get(2)?,
                    branch: row.get(3)?,
                    commits: Vec::new(),
                    files: Vec::new(),
                },
            ))
        })
        .sql()?;
    for row in rows {
        let (id, work) = row.sql()?;
        all.entry(id).or_default().push(work);
    }
    let mut statement = db
        .prepare("SELECT item_id, repo, sha, subject FROM dropped_commit ORDER BY item_id, repo, position")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                DroppedCommit {
                    sha: row.get(2)?,
                    subject: row.get(3)?,
                },
            ))
        })
        .sql()?;
    for row in rows {
        let (id, repo, commit) = row.sql()?;
        place_of(&mut all, id, &repo, "dropped_commit")?
            .commits
            .push(commit);
    }
    let mut statement = db
        .prepare("SELECT item_id, repo, path FROM dropped_file ORDER BY item_id, repo, path")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .sql()?;
    for row in rows {
        let (id, repo, path) = row.sql()?;
        place_of(&mut all, id, &repo, "dropped_file")?
            .files
            .push(path);
    }
    Ok(all)
}

/// The work of <all> that item <id> held in <repo>, for a row of <table>;
/// fails when there is none, since the table's rows belong to a
/// `dropped_place` row.
fn place_of<'a>(
    all: &'a mut BTreeMap<Id, Vec<DroppedWork>>,
    id: Id,
    repo: &str,
    table: &str,
) -> Result<&'a mut DroppedWork> {
    match all
        .get_mut(&id)
        .and_then(|works| works.iter_mut().find(|w| w.repo == repo))
    {
        Some(work) => Ok(work),
        None => {
            fail!("a {table} row of {id} names repo {repo}, where {id} has no dropped_place row")
        }
    }
}

/// Throws <work> away: detaches its worktree at origin/main, discarding the
/// files not committed (ignored files, such as the build links, stay),
/// deletes its branch and removes the worktree's claim. The worktree is then
/// free. The claim goes last, so no one takes the worktree before the rest
/// is done; a second run after a failure does again only what is left.
pub fn discard(work: &DroppedWork) -> Result<()> {
    let (_, main) = main_of(&work.worktree)?;
    let live = crate::lists::running::live_worktree(&work.worktree)?;
    if let Some(checkout) = &live {
        git_text(
            git(&checkout.top).args([
                "-c",
                "core.hooksPath=/dev/null",
                "checkout",
                "-q",
                "-f",
                "--detach",
                "origin/main",
            ]),
            "detach the worktree at origin/main",
        )?;
        git_text(
            git(&checkout.top).args(["clean", "-f", "-d", "-q"]),
            "remove the files not committed",
        )?;
    }
    if crate::checkout::branch_exists(&main, &work.branch)? {
        git_text(
            git(&main).args(["branch", "-q", "-D", &work.branch]),
            "delete the branch",
        )?;
    }
    if let Some(checkout) = &live {
        forget_claim(checkout)?;
    }
    Ok(())
}

fn forget_claim(checkout: &Checkout) -> Result<()> {
    crate::claim::forget(checkout).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dropped_commit row whose item has no dropped_place row in its repo
    /// is a broken record, which read_all names instead of leaving out.
    #[test]
    fn read_all_fails_on_a_commit_with_no_place() {
        let db = crate::lists::fixtures::db();
        let id = crate::lists::fixtures::item(&db, vec![]);
        db.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        db.execute(
            "INSERT INTO dropped_commit (item_id, repo, position, sha, subject) VALUES (?1, 'demo', 0, ?2, 's')",
            params![id, "a".repeat(40)],
        )
        .unwrap();
        let said = match read_all(&db) {
            Err(crate::fail::Fail::Message(message)) => message,
            other => panic!("{other:?}"),
        };
        assert!(said.contains("no dropped_place row"), "{said}");
    }
}
