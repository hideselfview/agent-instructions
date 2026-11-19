//! Each worktree's design commits (`design::design_commits`), read once for
//! each tip: the commits waiting to land, origin/main's commit, and the
//! version of the project's store, which holds the repo's design globs and
//! its open design changes. The dashboard's design cards and health's
//! findings about held work read them through one `DesignReads`, so an open
//! page asks git once for each tip, not on every health sample.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::fail::Result;
use crate::lists::Lists;
use crate::lists::design::{DesignCommit, design_commits};

/// What a worktree's design commits are read from besides the files of its
/// commits, which never change.
#[derive(PartialEq, Eq)]
struct Tip {
    /// The commits waiting to land, full hashes, oldest first.
    ahead: Vec<String>,
    /// origin/main's commit, which says whether a board a commit deletes is
    /// on origin/main.
    main: String,
    /// The version of the store whose lists name the design globs and the
    /// open design changes.
    store: u64,
}

/// A worktree's design commits and the tip they were read at.
type Read = (Tip, Arc<Vec<DesignCommit>>);

/// Each worktree's design commits, by the worktree's folder.
#[derive(Default)]
pub struct DesignReads {
    read: Mutex<HashMap<PathBuf, Read>>,
}

impl DesignReads {
    /// The design commits of <ahead> (full hashes, oldest first) in the
    /// worktree at <top> of <repo>, whose origin/main is at commit <main>,
    /// against the design globs and open design changes of <lists>: the ones
    /// kept for the worktree when they were read at the same tip, else read
    /// now. Keeps only a read that succeeded, so the next asker reads again
    /// after a failure.
    pub fn of(
        &self,
        lists: &Lists,
        repo: &str,
        top: &Path,
        ahead: &[String],
        main: &str,
    ) -> Result<Arc<Vec<DesignCommit>>> {
        let tip = Tip {
            ahead: ahead.to_vec(),
            main: main.to_string(),
            store: lists.version,
        };
        // Held while reading, so askers at once read once.
        let mut read = self.read.lock().expect("no panic holding it");
        if let Some((kept, commits)) = read.get(top)
            && *kept == tip
        {
            return Ok(commits.clone());
        }
        let commits = Arc::new(design_commits(lists, repo, top, ahead)?);
        read.insert(top.to_path_buf(), (tip, commits.clone()));
        Ok(commits)
    }
}

#[cfg(test)]
#[path = "design_reads_tests.rs"]
mod tests;
