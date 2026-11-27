//! Work that origin/main lacks. `builds release` and `builds retire` refuse a
//! worktree that holds such work which nothing else holds, and `builds new`
//! hands out no such worktree (`in_worktree`, which kept.rs calls for all
//! three; `retire` and `new` also refuse one whose HEAD has such work that
//! something else holds, `Held::past_main`), so a session can't drop the work by releasing. `builds health`
//! lists each local branch that holds such work while no worktree checks it
//! out and no open item names it (`loose_branches`, `unheld`). Work leaves a
//! worktree only by landing, by an item that holds it (running held on the
//! person, or kept), or by the person dropping that item
//! (lists/dropped.rs).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::Result;
use crate::side_git::Commit;
use boards::{git, git_text};

/// The work a worktree holds that origin/main lacks.
#[derive(Debug, Default)]
pub struct Held {
    /// The commits on HEAD, newest first, with HEAD's branch, or None when
    /// HEAD is detached.
    pub head: (Option<String>, Vec<Commit>),
    /// Each other branch the worktree had checked out since its claim began,
    /// with its commits, newest first.
    pub left: Vec<(String, Vec<Commit>)>,
    /// The commits on HEAD origin/main lacks, newest first, those of `head`
    /// and those another branch has a copy of.
    pub past_main: Vec<Commit>,
    /// The files not committed.
    pub uncommitted: Vec<String>,
    /// The pids of the `git commit`s that run in it: files git is still
    /// committing, which a pre-commit hook can make take minutes.
    pub committing: Vec<i32>,
    /// The branches that hold a copy (the same hash or the same patch) of
    /// commits the worktree has and that `head` and `left` leave out for
    /// that reason. Nothing is lost by releasing a worktree whose only
    /// commits are such copies.
    pub copied_on: BTreeSet<String>,
}

impl Held {
    pub fn is_empty(&self) -> bool {
        self.committing.is_empty() && self.git_is_empty()
    }

    pub fn git_is_empty(&self) -> bool {
        self.head.1.is_empty()
            && self.left.iter().all(|(_, commits)| commits.is_empty())
            && self.uncommitted.is_empty()
    }
}

/// The branches the worktree <top> left since <since> (seconds since 1970),
/// as its HEAD's reflog records each checkout: `checkout: moving from A to
/// B` names A, the branch left.
fn branches_left(top: &Path, since: i64) -> Result<Vec<String>> {
    let log = git_text(
        git(top).args([
            "reflog",
            "show",
            "HEAD",
            "--date=unix",
            "--format=%gd%x00%gs",
        ]),
        "read HEAD's reflog",
    )?;
    let mut left = Vec::new();
    for line in log.lines() {
        let Some((selector, subject)) = line.split_once('\0') else {
            continue;
        };
        let at = selector
            .strip_prefix("HEAD@{")
            .and_then(|rest| rest.strip_suffix('}'))
            .and_then(|at| at.parse::<i64>().ok());
        let Some(at) = at else {
            fail!("git printed a reflog entry {selector:?} with no time");
        };
        if at < since {
            break;
        }
        let Some(moved) = subject.strip_prefix("checkout: moving from ") else {
            continue;
        };
        let Some((from, _)) = moved.split_once(" to ") else {
            continue;
        };
        if !left.iter().any(|known| known == from) {
            left.push(from.to_string());
        }
    }
    Ok(left)
}

/// The local branches that can hold the commits of <checkout>: the branch
/// of each other worktree of its repo, and each name of <items> (the
/// branches open items run or are kept on), except the branch <checkout>
/// has checked out, <own>. <listed> is the worktree list the caller read, so
/// the caller and this function judge one answer. A commit that one of them
/// holds a copy of is held by it.
fn holders(
    checkout: &Checkout,
    own: Option<&String>,
    listed: &[crate::checkout::Listed],
    items: &[String],
) -> Result<Vec<String>> {
    let top = &checkout.top;
    let canonical = |path: &Path| {
        std::fs::canonicalize(path)
            .map_err(|error| fail::Fail::Message(format!("resolving {}: {error}", path.display())))
    };
    let here = canonical(top)?;
    let mut elsewhere = Vec::new();
    for other in listed {
        if canonical(&other.top)? != here {
            elsewhere.extend(other.branch.clone());
        }
    }
    let mut holders: Vec<String> = Vec::new();
    for name in elsewhere.into_iter().chain(items.iter().cloned()) {
        if Some(&name) != own && !holders.contains(&name) && crate::git::branch_exists(top, &name)?
        {
            holders.push(name);
        }
    }
    Ok(holders)
}

/// The branches open items of <every> run or keep in the repo of
/// <checkout>.
pub fn item_branches<'a>(
    every: impl Iterator<Item = &'a crate::lists::Lists>,
    checkout: &Checkout,
) -> Vec<String> {
    let repo = checkout.repo();
    named_branches(every)
        .into_iter()
        .filter(|(named_repo, _)| *named_repo == repo)
        .map(|(_, branch)| branch)
        .collect()
}

/// The work in <checkout> that origin/main lacks and no open item holds:
/// HEAD's commits, the commits of each branch the worktree left since
/// <since> (its claim's start) that no worktree checks out now and no name
/// of <held> (the branches open items run or are kept on) covers, the
/// files not committed, and the `git commit`s of <commits> (every one that
/// runs) that run there. A commit that a branch another worktree checks out,
/// or a name of <held>, has a copy of (the same patch) is no work: that
/// branch holds it (`Held::copied_on` names the branches). <known> is a read
/// of the worktree's git state the caller already has: its commits waiting
/// to land and its count of uncommitted files then replace those two reads.
pub fn in_worktree(
    checkout: &Checkout,
    since: i64,
    every: &[crate::lists::Lists],
    committing: Vec<i32>,
    known: Option<&crate::side_git::SideRead>,
) -> Result<Held> {
    let top = &checkout.top;
    let unread = |error: String| {
        fail::Fail::Message(format!("reading the work in {}: {error}", top.display()))
    };
    let branch = checkout.branch()?;
    let mut copied_on = BTreeSet::new();
    let held = item_branches(every.iter(), checkout);
    let listed = crate::checkout::worktree_list(top)?;
    let holders = holders(checkout, branch.as_ref(), &listed, &held)?;
    let mut split = |tip: &str, waiting: Vec<Commit>| -> Result<Vec<Commit>> {
        if waiting.is_empty() {
            return Ok(waiting);
        }
        let split = crate::side_git::split_held(top, tip, waiting, &holders).map_err(unread)?;
        copied_on.extend(split.copied_on);
        Ok(split.lacking)
    };
    let past_main = match known {
        Some(read) => read.ahead.clone(),
        None => {
            let tip = crate::app_config::resolve(top, "HEAD")?;
            crate::side_git::waiting_to_land(top, &tip).map_err(unread)?
        }
    };
    // The newest commit waiting to land has every other one among its
    // ancestors, so `git cherry` from it sees them all.
    let head = match past_main.first() {
        Some(newest) => split(&newest.full, past_main.clone())?,
        None => Vec::new(),
    };
    let checked_out: Vec<&String> = listed
        .iter()
        .filter_map(|listed| listed.branch.as_ref())
        .collect();
    let mut left = Vec::new();
    for name in branches_left(top, since)? {
        if Some(&name) == branch.as_ref() || checked_out.contains(&&name) || held.contains(&name) {
            continue;
        }
        let Some(tip) = crate::git::branch_tip(top, &name)? else {
            continue;
        };
        let waiting = crate::side_git::waiting_to_land(top, &tip).map_err(unread)?;
        let commits = split(&tip, waiting)?;
        if !commits.is_empty() {
            left.push((name, commits));
        }
    }
    Ok(Held {
        head: (branch, head),
        left,
        past_main,
        uncommitted: if known.is_some_and(|read| read.uncommitted == 0) {
            Vec::new()
        } else {
            crate::side_git::uncommitted_paths(top)
                .map_err(unread)?
                .into_iter()
                .collect()
        },
        committing,
        copied_on,
    })
}

/// What release says of the branches that hold copies of the worktree's
/// commits: empty when none does.
pub fn copies_words(branches: &BTreeSet<String>) -> String {
    if branches.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = branches.iter().map(String::as_str).collect();
    format!(
        "; its commits are copies of commits on {}, which keep them",
        names.join(", ")
    )
}

/// <commits> as a refusal lists them: each short sha and subject.
pub fn commit_words(commits: &[Commit]) -> String {
    commits
        .iter()
        .map(|c| format!("{} {}", c.sha, c.subject))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Why <what> (a command) refuses <top>, which holds <held>, and the ways
/// its work may leave the worktree.
pub fn refusal(what: &str, top: &Path, held: &Held) -> String {
    let top = top.display();
    let mut lines = vec![format!(
        "{what} refuses {top}: it holds work origin/main lacks, which nothing else holds:"
    )];
    let (branch, head) = &held.head;
    if !head.is_empty() {
        let on = match branch {
            Some(branch) => format!("branch {branch}"),
            None => "detached HEAD".to_string(),
        };
        lines.push(format!("  {on}: {}", commit_words(head)));
    }
    for (branch, commits) in &held.left {
        lines.push(format!(
            "  branch {branch}, which it left: {}",
            commit_words(commits)
        ));
    }
    if !held.uncommitted.is_empty() {
        lines.push(format!(
            "  {} not committed: {}",
            crate::text::counted(held.uncommitted.len(), "file", "files"),
            held.uncommitted.join(", ")
        ));
    }
    if !held.committing.is_empty() {
        lines.push(format!(
            "  a git commit runs in it (pid {}); wait for it to end",
            held.committing
                .iter()
                .map(|pid| pid.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let branch = branch.as_deref().unwrap_or("BRANCH");
    lines.push(format!(
        "Land it (`builds land {top} --done N SHA...`), or give it to an item: take one there (`builds item take N --worktree {top} --branch {branch} --brief P`, with `--held WHY` when the item waits on something open). Commit the files and give detached commits a branch (`git -C {top} switch -c NAME`) first. Only the person throws work away, by dropping its item."
    ));
    lines.join("\n")
}

/// A local branch that holds commits origin/main lacks while no worktree
/// checks it out: work nothing in builds shows unless an open item names it.
#[derive(Clone, Debug)]
pub struct LooseBranch {
    pub repo: String,
    pub branch: String,
    /// Newest first.
    pub commits: Vec<Commit>,
}

/// What `loose_branches` found of each branch tip against origin/main's tip.
/// The caller keeps it between readings, so `loose_branches` judges a branch
/// again only when its tip or origin/main's tip moves.
#[derive(Clone, Default)]
pub struct Judged(HashMap<(String, String), Vec<Commit>>);

/// Every local branch of the repo at <main> whose tip origin/main does not
/// hold, with its tip's commit id, in branch order. Some of them hold only
/// commits origin/main has copies of.
pub fn unmerged_branches(main: &Path) -> Result<Vec<(String, String)>> {
    let listed = git_text(
        git(main).args([
            "for-each-ref",
            "--format=%(refname:short)%00%(objectname)",
            "--no-merged=origin/main",
            "refs/heads",
        ]),
        "list the branches",
    )?;
    let mut branches = Vec::new();
    for line in listed.lines() {
        let Some((branch, sha)) = line.split_once('\0') else {
            fail!(
                "git for-each-ref printed {line:?}, with no NUL between the branch and its commit"
            );
        };
        branches.push((branch.to_string(), sha.to_string()));
    }
    Ok(branches)
}

/// Every local branch of the repo whose main checkout is <main> that holds
/// commits origin/main lacks and that no worktree checks out, in branch
/// order. <judged> keeps what earlier readings found.
pub fn loose_branches(main: &Path, judged: &mut Judged) -> Result<Vec<LooseBranch>> {
    let repo = crate::checkout::file_name(main);
    let tip = crate::app_config::resolve(main, "origin/main")?;
    let checked_out: BTreeSet<String> = crate::checkout::worktree_list(main)?
        .into_iter()
        .filter_map(|listed| listed.branch)
        .collect();
    let branches: Vec<(String, String)> = unmerged_branches(main)?
        .into_iter()
        .filter(|(branch, _)| !checked_out.contains(branch))
        .collect();
    let unjudged: Vec<&(String, String)> = branches
        .iter()
        .filter(|(_, sha)| !judged.0.contains_key(&(sha.clone(), tip.clone())))
        .collect();
    if !unjudged.is_empty() {
        let copies = crate::side_git::MainCopies::all(main).map_err(fail::Fail::Message)?;
        let found = crate::checkout::parallel_map(&unjudged, |(_, sha)| {
            crate::side_git::waiting_to_land_beside(main, sha, &copies)
        });
        for ((_, sha), commits) in unjudged.iter().zip(found) {
            let commits = commits.map_err(fail::Fail::Message)?;
            judged.0.insert((sha.clone(), tip.clone()), commits);
        }
    }
    let kept: BTreeSet<(String, String)> = branches
        .iter()
        .map(|(_, sha)| (sha.clone(), tip.clone()))
        .collect();
    judged.0.retain(|key, _| kept.contains(key));
    Ok(branches
        .into_iter()
        .filter_map(|(branch, sha)| {
            let commits = judged.0[&(sha, tip.clone())].clone();
            (!commits.is_empty()).then(|| LooseBranch {
                repo: repo.clone(),
                branch,
                commits,
            })
        })
        .collect())
}

/// The branches of <loose> that no open item of <loaded> names: an item's
/// running or kept place names its repo and branch. A store that does not
/// load names nothing, and is a finding of its own.
pub fn unheld(
    loose: Vec<LooseBranch>,
    loaded: &std::result::Result<Vec<crate::lists::store::Loaded>, String>,
) -> Vec<LooseBranch> {
    let named = named_branches(
        loaded
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|project| project.lists.as_ref().ok()),
    );
    loose
        .into_iter()
        .filter(|b| !named.contains(&(b.repo.clone(), b.branch.clone())))
        .collect()
}

/// The repo and branch of every running or kept place of an open item of
/// <lists>: the branches items hold.
pub fn named_branches<'a>(
    lists: impl Iterator<Item = &'a crate::lists::Lists>,
) -> BTreeSet<(String, String)> {
    let mut named = BTreeSet::new();
    for lists in lists {
        for item in &lists.items {
            let places = item
                .running
                .iter()
                .map(|r| (&r.worktree, &r.branch))
                .chain(item.kept.iter().map(|p| (&p.worktree, &p.branch)));
            for (worktree, branch) in places {
                if let Some((repo, _)) = crate::lists::side_worktree_parts(worktree) {
                    named.insert((repo, branch.clone()));
                }
            }
        }
    }
    named
}
