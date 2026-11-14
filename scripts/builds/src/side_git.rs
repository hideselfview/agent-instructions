//! What git says about a side worktree: HEAD, the commits waiting to land,
//! and its uncommitted files. Read-only, with git's optional locks off, and
//! nothing fetched. Main checkouts are the person's; nothing asks git about
//! them here.

use std::collections::HashSet;
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

fn run(top: &Path, args: &[&str]) -> Result<String, String> {
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

/// The commits on HEAD that origin/main has no copy of, newest first.
///
/// A coordinator lands a branch by cherry-picking or rebasing it, so the
/// branch's own commits stay off origin/main by hash after they land. A copy
/// is either a commit with the same patch (`git cherry` marks it "-"), or,
/// when the landing resolved a conflict or sat beside a changed neighbour
/// line so the patch differs, a commit on origin/main since the branch left
/// it with the same author time and subject, which a cherry-pick and a rebase
/// both keep.
fn waiting_to_land(top: &Path) -> Result<Vec<Commit>, String> {
    let marks = run(top, &["cherry", "origin/main", "HEAD"])?;
    let unmatched: HashSet<&str> = marks
        .lines()
        .filter_map(|line| line.strip_prefix("+ "))
        .collect();
    let landed: HashSet<(i64, String)> = commits(top, &["HEAD..origin/main"])?
        .into_iter()
        .map(|c| (c.authored, c.subject))
        .collect();
    Ok(commits(top, &["origin/main..HEAD"])?
        .into_iter()
        .filter(|c| {
            unmatched.contains(c.full.as_str())
                && !landed.contains(&(c.authored, c.subject.clone()))
        })
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

/// HEAD, the commits waiting to land, the uncommitted count and their newest
/// write in the side worktree at <top>; on a failure, what git said.
pub fn read(top: &Path) -> SideGit {
    let read = || -> Result<SideGit, String> {
        let head = commits(top, &["-1", "HEAD"])?.into_iter().next();
        let (count, newest) = uncommitted(top)?;
        Ok(SideGit {
            head,
            ahead: waiting_to_land(top)?,
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
