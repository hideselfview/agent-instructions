//! Work that origin/main lacks. `builds release` and `builds retire` refuse a
//! worktree that holds such work which nothing else holds, and `builds new`
//! hands out no such worktree (`in_worktree`, which kept.rs calls for all
//! three), so a session can't drop the work by releasing. `builds health`
//! lists each local branch that holds such work while no worktree checks it
//! out and no open item names it (`loose_branches`, `unheld`). Work leaves a
//! worktree only by landing, by an item that holds it (running held on the
//! person, or parked), or by the person dropping that item
//! (lists/dropped.rs).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::checkout::{Checkout, git, git_text};
use crate::fail;
use crate::fail::Result;
use crate::side_git::Commit;

/// The work a worktree holds that origin/main lacks.
#[derive(Debug, Default)]
pub struct Held {
    /// The commits on HEAD, newest first, with HEAD's branch, or None when
    /// HEAD is detached.
    pub head: (Option<String>, Vec<Commit>),
    /// Each other branch the worktree had checked out since its claim began,
    /// with its commits, newest first.
    pub left: Vec<(String, Vec<Commit>)>,
    /// The files not committed.
    pub uncommitted: Vec<String>,
    /// The pids of the `git commit`s that run in it: files git is still
    /// committing, which a pre-commit hook can make take minutes.
    pub committing: Vec<i32>,
}

impl Held {
    pub fn is_empty(&self) -> bool {
        self.head.1.is_empty()
            && self.left.iter().all(|(_, commits)| commits.is_empty())
            && self.uncommitted.is_empty()
            && self.committing.is_empty()
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

/// The work in <checkout> that origin/main lacks and no open item holds:
/// HEAD's commits, the commits of each branch the worktree left since
/// <since> (its claim's start) that no worktree checks out now and no name
/// of <held> (the branches open items run or are parked on) covers, the
/// files not committed, and the `git commit`s of <commits> (every one that
/// runs) that run there.
pub fn in_worktree(
    checkout: &Checkout,
    since: i64,
    held: &[String],
    commits: &crate::procs::GitCommits,
) -> Result<Held> {
    let top = &checkout.top;
    let unread = |error: String| {
        fail::Fail::Message(format!("reading the work in {}: {error}", top.display()))
    };
    let branch = checkout.branch()?;
    let head = crate::side_git::waiting_to_land(top, "HEAD").map_err(unread)?;
    let checked_out: Vec<String> = crate::checkout::worktree_list(top)?
        .into_iter()
        .filter_map(|listed| listed.branch)
        .collect();
    let mut left = Vec::new();
    for name in branches_left(top, since)? {
        if Some(&name) == branch.as_ref()
            || checked_out.contains(&name)
            || held.contains(&name)
            || !crate::checkout::branch_exists(top, &name)?
        {
            continue;
        }
        let commits =
            crate::side_git::waiting_to_land(top, &format!("refs/heads/{name}")).map_err(unread)?;
        if !commits.is_empty() {
            left.push((name, commits));
        }
    }
    Ok(Held {
        head: (branch, head),
        left,
        uncommitted: crate::side_git::uncommitted_paths(top)
            .map_err(unread)?
            .into_iter()
            .collect(),
        committing: crate::procs::commits_in(commits, top).map_err(fail::Fail::Message)?,
    })
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
        "Land it (`builds land {top} --done N SHA...`), park it under its item (`builds item park N --worktree {top} --branch {branch}`), or hand it to an item held on the person (`builds item take N --worktree {top} --branch {branch} --brief P --held WHY`). Commit the files and give detached commits a branch (`git -C {top} switch -c NAME`) first. Only the person throws work away, by dropping its item."
    ));
    lines.join("\n")
}

/// A local branch that holds commits origin/main lacks while no worktree
/// checks it out: work nothing in builds shows unless an open item names it.
#[derive(Clone, Debug)]
pub struct LooseBranch {
    pub repo: String,
    /// The repo's main checkout.
    pub main: PathBuf,
    pub branch: String,
    /// Newest first.
    pub commits: Vec<Commit>,
}

/// What `loose_branches` found of each branch tip against origin/main's tip.
/// The caller keeps it between readings, so `loose_branches` judges a branch
/// again only when its tip or origin/main's tip moves.
#[derive(Default)]
pub struct Judged(HashMap<(String, String), Vec<Commit>>);

/// Every local branch of the repo whose main checkout is <main> that holds
/// commits origin/main lacks and that no worktree checks out, in branch
/// order. <judged> keeps what earlier readings found.
pub fn loose_branches(main: &Path, judged: &mut Judged) -> Result<Vec<LooseBranch>> {
    let repo = crate::checkout::file_name(main);
    let tip = git_text(
        git(main).args(["rev-parse", "--verify", "origin/main^{commit}"]),
        "read origin/main",
    )?
    .trim()
    .to_string();
    let checked_out: BTreeSet<String> = crate::checkout::worktree_list(main)?
        .into_iter()
        .filter_map(|listed| listed.branch)
        .collect();
    let listed = git_text(
        git(main).args([
            "for-each-ref",
            "--format=%(refname:short)%00%(objectname)",
            "--no-merged=origin/main",
            "refs/heads",
        ]),
        "list the branches",
    )?;
    let mut branches: Vec<(String, String)> = Vec::new();
    for line in listed.lines() {
        let Some((branch, sha)) = line.split_once('\0') else {
            fail!(
                "git for-each-ref printed {line:?}, with no NUL between the branch and its commit"
            );
        };
        if !checked_out.contains(branch) {
            branches.push((branch.to_string(), sha.to_string()));
        }
    }
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
                main: main.to_path_buf(),
                branch,
                commits,
            })
        })
        .collect())
}

/// The branches of <loose> that no open item of <loaded> names: an item's
/// running or parked place names its repo and branch. A store that does not
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

/// The repo and branch of every running or parked place of an open item of
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
                .chain(item.parked.iter().map(|p| (&p.worktree, &p.branch)));
            for (worktree, branch) in places {
                if let Some((repo, _)) = crate::lists::side_worktree_parts(worktree) {
                    named.insert((repo, branch.clone()));
                }
            }
        }
    }
    named
}
