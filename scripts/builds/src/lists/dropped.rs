//! The work an item held when the person dropped it. Dropping an item that
//! runs or is parked throws its work away (`builds item drop`): the drop
//! records each worktree's branch, the commits on it and on HEAD that
//! origin/main lacks and the files that were not committed, then detaches the
//! worktree at origin/main, discards its changes, deletes the branch and
//! removes the worktree's claim, all inside the store's write, so a git
//! failure leaves the item open and the record unwritten.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::checkout::{Checkout, git, git_text};
use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::Id;
use crate::lists::running::side_worktree_split;
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
    let Some((parent, repo, _)) = side_worktree_split(worktree) else {
        fail!("{worktree} is not a side worktree (<parent>/<repo>-worktrees/side-N)");
    };
    let main = Path::new(&parent).join(&repo);
    Ok((repo, main))
}

/// Does branch <branch> exist in the repo of <dir>?
fn branch_exists(dir: &Path, branch: &str) -> Result<bool> {
    let status = git(dir)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| {
            fail::Fail::Message(format!("running git in {}: {error}", dir.display()))
        })?;
    Ok(status.success())
}

/// What <worktree> holds of an item's work on <branch>: the commits on the
/// branch and on the worktree's HEAD that origin/main lacks, and the files
/// not committed there. Refused when another worktree checks the branch out,
/// since deleting it would take that worktree's work too.
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
    if branch_exists(&main, branch)? {
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

/// Records <works> as what closed item <id> held when it was dropped.
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
        if let Some(work) = all
            .get_mut(&id)
            .and_then(|works| works.iter_mut().find(|w| w.repo == repo))
        {
            work.commits.push(commit);
        }
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
        if let Some(work) = all
            .get_mut(&id)
            .and_then(|works| works.iter_mut().find(|w| w.repo == repo))
        {
            work.files.push(path);
        }
    }
    Ok(all)
}

/// Throws <work> away: detaches its worktree at origin/main, discarding the
/// files not committed (ignored files, such as the build links, stay),
/// deletes its branch and removes the worktree's claim. The worktree is then
/// free.
pub fn discard(work: &DroppedWork) -> Result<()> {
    let (_, main) = main_of(&work.worktree)?;
    if let Some(checkout) = crate::lists::running::live_worktree(&work.worktree)? {
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
        forget_claim(&checkout)?;
    }
    if branch_exists(&main, &work.branch)? {
        git_text(
            git(&main).args(["branch", "-q", "-D", &work.branch]),
            "delete the branch",
        )?;
    }
    Ok(())
}

fn forget_claim(checkout: &Checkout) -> Result<()> {
    crate::claim::forget(checkout).map(|_| ())
}
