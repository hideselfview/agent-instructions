//! Each worktree's design commits (`design::design_commits`), read once for
//! each tip: the commits waiting to land, origin/main's commit, and the
//! version of the project's store, which holds the repo's design globs and
//! its open design changes. The dashboard's design cards and health's
//! findings about held work read them through one `DesignReads`, so an open
//! page asks git once for each tip, not on every health sample.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::fail::{Fail, Result};
use crate::lists::Lists;
use crate::lists::design::{DesignCommit, design_commits};
use crate::lists::design_drawn::{ProgramRenders, Renders};
use crate::side_git::SideRead;

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
    project: String,
}

impl Tip {
    fn of(lists: &Lists, git: &SideRead) -> Self {
        Self {
            ahead: git.ahead_oldest_first(),
            main: git.main.clone(),
            store: lists.version,
            project: lists.project.clone(),
        }
    }
}

/// A worktree's design commits at one tip. Only readers of this tip wait on
/// its Git and renderer processes.
struct Read {
    tip: Tip,
    reading: Mutex<()>,
    outcome: RwLock<Option<Outcome>>,
}

type Outcome = std::result::Result<Arc<Vec<DesignCommit>>, String>;

/// Each worktree's design commits, by the worktree's folder.
pub struct DesignReads {
    read: Mutex<HashMap<PathBuf, Arc<Read>>>,
    changed: OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// Draws the boards of a change when a commit's files differ from the
    /// change's (`Sameness`).
    renders: Box<dyn Renders>,
}

/// Reads whose files' bytes alone decide, for the tests that make no
/// drawings.
#[cfg(test)]
impl Default for DesignReads {
    fn default() -> Self {
        Self::new(Box::new(crate::lists::design_drawn::NeverAlike))
    }
}

impl DesignReads {
    /// Reads that draw boards with <renders>.
    pub fn new(renders: Box<dyn Renders>) -> Self {
        Self {
            read: Mutex::default(),
            changed: OnceLock::new(),
            renders,
        }
    }

    /// Reads that draw boards with the builds program at <program>, keeping
    /// each answer in the caches of <places>.
    pub fn with_program(places: &crate::places::Places, program: PathBuf) -> Self {
        Self::new(Box::new(ProgramRenders::of(places, program)))
    }

    /// Wakes the dashboard when the result for a tip changes.
    pub fn listen(&self, changed: impl Fn() + Send + Sync + 'static) {
        assert!(self.changed.set(Box::new(changed)).is_ok());
    }

    /// The result at this tip if a read has finished. Never waits for Git or
    /// a renderer. A failure is a result that the dashboard can show.
    pub fn completed(
        &self,
        lists: &Lists,
        repo: &str,
        top: &Path,
        git: &SideRead,
    ) -> Option<Outcome> {
        if Self::has_nothing_to_read(lists, repo, git) {
            return Some(Ok(Arc::default()));
        }
        let tip = Tip::of(lists, git);
        let read = self
            .read
            .lock()
            .expect("no panic holding it")
            .get(top)?
            .clone();
        if read.tip != tip {
            return None;
        }
        read.outcome.read().expect("no panic holding it").clone()
    }

    /// Reads the tip for the dashboard if no other reader has finished it.
    pub fn fill(&self, lists: &Lists, repo: &str, top: &Path, git: &SideRead) {
        if !Self::has_nothing_to_read(lists, repo, git) {
            // read_at publishes failures as well as successes.
            drop(self.read_at(lists, repo, top, git, false));
        }
    }

    fn has_nothing_to_read(lists: &Lists, repo: &str, git: &SideRead) -> bool {
        git.ahead.is_empty()
            || (lists.design_places.get(repo).is_none_or(Vec::is_empty)
                && lists.design_sources.get(repo).is_none_or(Vec::is_empty))
    }

    /// The design commits in the worktree at <top> of <repo>, whose side
    /// read is <git>, against the design globs and open design changes of
    /// <lists>: the ones
    /// kept for the worktree when they were read at the same tip, else read
    /// now. An explicit read retries a previous failure.
    pub fn of(
        &self,
        lists: &Lists,
        repo: &str,
        top: &Path,
        git: &SideRead,
    ) -> Result<Arc<Vec<DesignCommit>>> {
        if Self::has_nothing_to_read(lists, repo, git) {
            return Ok(Arc::default());
        }
        self.read_at(lists, repo, top, git, true)
            .map_err(Fail::Message)
    }

    fn read_at(
        &self,
        lists: &Lists,
        repo: &str,
        top: &Path,
        git: &SideRead,
        retry_failed: bool,
    ) -> Outcome {
        let tip = Tip::of(lists, git);
        let read = {
            let mut by_worktree = self.read.lock().expect("no panic holding it");
            match by_worktree.get(top).filter(|kept| kept.tip == tip) {
                Some(kept) => kept.clone(),
                None => {
                    let read = Arc::new(Read {
                        tip,
                        reading: Mutex::new(()),
                        outcome: RwLock::new(None),
                    });
                    by_worktree.insert(top.to_path_buf(), read.clone());
                    read
                }
            }
        };
        let reading = read.reading.lock().expect("no panic holding it");
        if let Some(kept) = read.outcome.read().expect("no panic holding it").as_ref()
            && (!retry_failed || kept.is_ok())
        {
            return kept.clone();
        }
        let found = design_commits(lists, repo, top, &read.tip.ahead, self.renders.as_ref())
            .map(Arc::new)
            .map_err(Fail::text);
        let changed = {
            let mut kept = read.outcome.write().expect("no panic holding it");
            let repeated_failure =
                matches!((&*kept, &found), (Some(Err(old)), Err(new)) if old == new);
            *kept = Some(found.clone());
            !repeated_failure
        };
        drop(reading);
        if changed && let Some(listener) = self.changed.get() {
            listener();
        }
        found
    }
}

#[cfg(test)]
#[path = "design_reads_tests.rs"]
mod tests;
