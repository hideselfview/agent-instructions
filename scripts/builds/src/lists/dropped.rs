//! The work an item held when the person dropped it. Dropping an item that
//! runs or is kept alone in its worktrees throws its work away (`builds
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

use crate::checkout::Checkout;
use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::item::open_item;
use crate::lists::kept_work;
use crate::lists::record::close;
use crate::lists::running::{self, require_side_worktree, running_in};
use crate::lists::{How, Id, Lists, Stamp};
use crate::side_git::Commit;
use boards::{git, git_text};

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
/// <every> (each project's lists) runs or is kept in one of <worktrees>:
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
            let kept = crate::lists::kept_work::kept_in(lists, worktree);
            for item in running_in(lists, worktree).into_iter().chain(kept) {
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

/// Drops open item <id> of the project that <lists> holds, inside a store
/// write: refuses while another open item of <others> (the other projects'
/// lists) or of <lists> runs or is kept in one of its worktrees, reads the
/// work in each of them, ends its runs and its keeping, closes it as dropped
/// with <why> and records the work. Returns the work to throw away with
/// `discard_all` once the write commits.
pub fn drop_open_item(
    db: &Connection,
    lists: &Lists,
    others: &[Lists],
    id: Id,
    why: &str,
    stamp: &Stamp,
) -> Result<Vec<DroppedWork>> {
    let item = open_item(lists, id)?;
    let places: Vec<(&str, &str)> = item
        .running
        .iter()
        .map(|r| (r.worktree.as_str(), r.branch.as_str()))
        .chain(
            item.kept
                .iter()
                .map(|p| (p.worktree.as_str(), p.branch.as_str())),
        )
        .collect();
    let worktrees: Vec<&str> = places.iter().map(|(worktree, _)| *worktree).collect();
    require_alone(others.iter().chain([lists]), &lists.project, id, &worktrees)?;
    let works = places
        .iter()
        .map(|(worktree, branch)| read(worktree, branch))
        .collect::<Result<Vec<_>>>()?;
    running::end(db, id, running::Ended::Closed, stamp)?;
    kept_work::end(db, id)?;
    close(db, id, How::Dropped(why.to_string()), stamp)?;
    record(db, id, &works)?;
    Ok(works)
}

/// The lists of every project but <project>, which a drop reads because an
/// item of another project may run in a worktree the dropped item keeps.
pub fn other_projects(places: &crate::places::Places, project: &str) -> Result<Vec<Lists>> {
    Ok(crate::lists::store::every_lists(places)?
        .into_iter()
        .filter(|lists| lists.project != project)
        .collect())
}

/// Throws away the work of each of <dropped> (item, recorded work), trying
/// every item even when one fails; the failure names each item whose work
/// stays and the drop that finishes it.
pub fn discard_each(dropped: &[(Id, Vec<DroppedWork>)]) -> Result<()> {
    let failures: Vec<String> = dropped
        .iter()
        .filter_map(|(id, works)| discard_all(*id, works).err())
        .map(|error| error.text())
        .collect();
    if !failures.is_empty() {
        fail!("{}", failures.join("\n"));
    }
    Ok(())
}

/// Throws away <works>, the recorded work of dropped item <id>; a failure
/// names the drop to run again.
pub fn discard_all(id: Id, works: &[DroppedWork]) -> Result<()> {
    for work in works {
        if let Err(error) = discard(work) {
            fail!(
                "{id} is dropped and its work recorded, but throwing away the work in {} failed: {}; run `builds item drop {} --why WHY` again once that is fixed",
                work.worktree,
                error.text(),
                id.0
            );
        }
    }
    Ok(())
}

/// The line that says what the drop threw away in <work>.
pub fn thrown_away_line(work: &DroppedWork) -> String {
    format!(
        "threw away the work in {} on {}: {} and {}",
        work.worktree,
        work.branch,
        crate::text::counted(work.commits.len(), "commit", "commits"),
        crate::text::counted(
            work.files.len(),
            "file not committed",
            "files not committed"
        )
    )
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
