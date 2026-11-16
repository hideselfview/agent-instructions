//! What git says about a side worktree: HEAD, the commits waiting to land,
//! and its uncommitted files. Read-only, with git's optional locks off, and
//! nothing fetched. Main checkouts are the person's; nothing here asks git
//! about them, and main_checkout.rs reads their refs through `run`.

use std::collections::{BTreeSet, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

/// One commit, as the page and `health` show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub full: String,
    /// Commit time, seconds since 1970.
    pub time: i64,
    /// Author time, which a cherry-pick and a rebase keep.
    pub authored: i64,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideGit {
    pub head: Option<Commit>,
    /// The commits on HEAD origin/main has no copy of, newest first.
    pub ahead: Vec<Commit>,
    pub uncommitted: Option<usize>,
    /// The newest modification time among the uncommitted files that still
    /// exist.
    pub last_write: Option<i64>,
    /// What git said when a read failed.
    pub error: Option<String>,
}

impl SideGit {
    /// The newer of HEAD's commit time and the newest uncommitted write: the
    /// worktree's last activity.
    pub fn activity(&self) -> Option<i64> {
        let head = self.head.as_ref().map(|c| c.time);
        match (head, self.last_write) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

/// What git printed in <top>, run read-only with its optional locks off, or
/// what it said when it failed.
pub fn run(top: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(top)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir("/")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("running git: {error}"))?;
    if !output.status.success() {
        let shown: Vec<&str> = args.iter().take(4).copied().collect();
        return Err(format!(
            "git -C {} {} exited {}: {}",
            top.display(),
            shown.join(" "),
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The commits `git log <rev>` lists in <top>, newest first.
fn commits(top: &Path, rev: &[&str]) -> Result<Vec<Commit>, String> {
    let mut args = vec!["log", "--format=%h%x00%H%x00%ct%x00%at%x00%s%x00"];
    args.extend_from_slice(rev);
    let out = run(top, &args)?;
    let fields: Vec<&str> = out.split('\0').collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + 4 < fields.len() {
        found.push(Commit {
            sha: fields[i].trim().to_string(),
            full: fields[i + 1].to_string(),
            time: fields[i + 2]
                .parse()
                .map_err(|_| format!("git log printed a time {:?}", fields[i + 2]))?,
            authored: fields[i + 3]
                .parse()
                .map_err(|_| format!("git log printed a time {:?}", fields[i + 3]))?,
            subject: fields[i + 4].to_string(),
        });
        i += 5;
    }
    Ok(found)
}

/// The commits on origin/main by author time and subject, which a
/// cherry-pick and a rebase both keep. `waiting_to_land` uses them to find a
/// landed copy of a commit whose patch changed when it landed.
pub struct MainCopies(HashSet<(i64, String)>);

impl MainCopies {
    /// The commits on origin/main since <rev> left it: enough to judge <rev>.
    fn since(top: &Path, rev: &str) -> Result<MainCopies, String> {
        MainCopies::of(top, &format!("{rev}..origin/main"))
    }

    /// Every commit on origin/main: enough to judge any rev, so a caller
    /// that checks many revs reads origin/main once.
    pub fn all(top: &Path) -> Result<MainCopies, String> {
        MainCopies::of(top, "origin/main")
    }

    fn of(top: &Path, range: &str) -> Result<MainCopies, String> {
        Ok(MainCopies(
            commits(top, &[range])?
                .into_iter()
                .map(|c| (c.authored, c.subject))
                .collect(),
        ))
    }

    fn holds(&self, commit: &Commit) -> bool {
        self.0.contains(&(commit.authored, commit.subject.clone()))
    }
}

/// The commits on <rev> that origin/main has no copy of, newest first.
///
/// A coordinator lands a branch by cherry-picking or rebasing it, so the
/// branch's own commits stay off origin/main by hash after they land. A copy
/// is either a commit with the same patch (`git cherry` marks it "-"), or,
/// when the landing resolved a conflict or sat beside a changed neighbour
/// line so the patch differs, a commit on origin/main since the branch left
/// it with the same author time and subject, which a cherry-pick and a rebase
/// both keep.
pub fn waiting_to_land(top: &Path, rev: &str) -> Result<Vec<Commit>, String> {
    waiting_to_land_beside(top, rev, &MainCopies::since(top, rev)?)
}

/// `waiting_to_land`, judged against <copies>, which holds at least the
/// commits on origin/main since <rev> left it. `git cherry` reads the patch
/// of every commit on both sides, which takes most of the time for a branch
/// far behind origin/main, so it runs only when author time and subject
/// leave a commit unmatched.
pub fn waiting_to_land_beside(
    top: &Path,
    rev: &str,
    copies: &MainCopies,
) -> Result<Vec<Commit>, String> {
    let unmatched: Vec<Commit> = commits(top, &[&format!("origin/main..{rev}")])?
        .into_iter()
        .filter(|c| !copies.holds(c))
        .collect();
    if unmatched.is_empty() {
        return Ok(unmatched);
    }
    let marks = run(top, &["cherry", "origin/main", rev])?;
    let no_patch_copy: HashSet<&str> = marks
        .lines()
        .filter_map(|line| line.strip_prefix("+ "))
        .collect();
    Ok(unmatched
        .into_iter()
        .filter(|c| no_patch_copy.contains(c.full.as_str()))
        .collect())
}

/// The count of changed and untracked files in <top>, and the newest
/// modification time among them. A directory git lists as untracked counts by
/// its own time; a deleted file has no time.
fn uncommitted(top: &Path) -> Result<(usize, Option<i64>), String> {
    let out = run(top, &["status", "--porcelain", "-z"])?;
    let entries: Vec<&str> = out.split('\0').collect();
    let mut count = 0;
    let mut newest: Option<i64> = None;
    let mut i = 0;
    while i < entries.len() {
        let entry = entries[i];
        i += 1;
        if entry.is_empty() {
            continue;
        }
        count += 1;
        if let Some(path) = entry.get(3..)
            && let Ok(meta) = std::fs::symlink_metadata(top.join(path))
        {
            newest = Some(newest.map_or(meta.mtime(), |n| n.max(meta.mtime())));
        }
        // A rename or copy names its source in the entry after it.
        let bytes = entry.as_bytes();
        if matches!(bytes.first(), Some(b'R' | b'C')) || matches!(bytes.get(1), Some(b'R' | b'C')) {
            i += 1;
        }
    }
    Ok((count, newest))
}

/// The paths `git status` calls dirty in <top>: staged, changed and
/// untracked alike. An untracked folder shows as each file in it.
pub fn uncommitted_paths(top: &Path) -> Result<BTreeSet<String>, String> {
    let status = run(
        top,
        &["status", "--porcelain", "-z", "-uall", "--no-renames"],
    )?;
    Ok(status
        .split('\0')
        .filter(|entry| entry.len() > 3)
        .map(|entry| entry[3..].to_string())
        .collect())
}

/// Only HEAD's commit, for a main checkout: its working tree is the person's
/// and is never asked about, so no uncommitted files, writes or commits
/// waiting to land.
pub fn read_head(top: &Path) -> SideGit {
    match commits(top, &["-1", "HEAD"]) {
        Ok(found) => SideGit {
            head: found.into_iter().next(),
            ahead: Vec::new(),
            uncommitted: None,
            last_write: None,
            error: None,
        },
        Err(error) => SideGit {
            head: None,
            ahead: Vec::new(),
            uncommitted: None,
            last_write: None,
            error: Some(error),
        },
    }
}

/// HEAD, the commits waiting to land, the uncommitted count and their newest
/// write in the side worktree at <top>; on a failure, what git said.
pub fn read(top: &Path) -> SideGit {
    let read = || -> Result<SideGit, String> {
        let head = commits(top, &["-1", "HEAD"])?.into_iter().next();
        let (count, newest) = uncommitted(top)?;
        Ok(SideGit {
            head,
            ahead: waiting_to_land(top, "HEAD")?,
            uncommitted: Some(count),
            last_write: newest,
            error: None,
        })
    };
    read().unwrap_or_else(|error| SideGit {
        head: None,
        ahead: Vec::new(),
        uncommitted: None,
        last_write: None,
        error: Some(error),
    })
}

#[cfg(test)]
#[path = "side_git_tests.rs"]
mod tests;
