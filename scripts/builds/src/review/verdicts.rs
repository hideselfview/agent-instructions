//! The verdict of the review on a worktree's commits, judged the way
//! `builds land` judges them (`landing::judge`), and kept so that the
//! dashboard's cards and health's findings read one judgment.
//!
//! Judging asks git for the branch's net change, so `Reads` judges a worktree
//! once for each tip: its branch, HEAD and origin/main's commit, the version
//! of its project's store, and the rules. It judges again only when one of
//! those differs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::fail::Fail;
use crate::lists::Lists;
use crate::lists::design_paths::Globs;
use crate::lists::store::Store;
use crate::places::Places;
use crate::review::commits;
use crate::review::landing::{self, CodeCheck, Need, Verdict};
use crate::review::record::Change;
use crate::review::run::{Material, Scope};
use crate::side_git::SideRead;

/// A worktree and its commits (full hashes, oldest first).
type ReadKey = (PathBuf, Vec<String>);

/// The paths each commit of a worktree changes.
type Read = Vec<Vec<String>>;

/// A project's rules as `Material::load` last read them, or why it failed,
/// and the count of changes to the rules files the read came after. A read
/// uses that count to decide whether the rules need reading again.
type Rules = (u64, Result<Arc<Material>, String>);

/// A side worktree whose card shows where its rules review stands, as the
/// daemon last read it.
pub struct Worktree<'a> {
    pub repo: &'a str,
    pub top: &'a Path,
    /// The branch HEAD is on; None when HEAD is detached.
    pub branch: Option<&'a str>,
    pub git: &'a SideRead,
}

/// What a worktree's standing is judged from besides the rules. HEAD and
/// origin/main's commit fix the commits waiting to land and the branch's net
/// change; the branch names the review's change; the store's version moves
/// with every write to the review's records.
#[derive(Clone, PartialEq, Eq)]
struct Tip {
    branch: Option<String>,
    head: Option<String>,
    main: String,
    store: u64,
}

impl Tip {
    /// <worktree>'s tip against a store at <version>.
    fn of(worktree: &Worktree, version: u64) -> Tip {
        Tip {
            branch: worktree.branch.map(str::to_string),
            head: worktree.git.head.as_ref().map(|head| head.full.clone()),
            main: worktree.git.main.clone(),
            store: version,
        }
    }
}

/// A worktree's verdict and what it was judged from.
struct Judged {
    tip: Tip,
    rules_version: u64,
    verdict: Outcome,
}

type Outcome = Result<Arc<Verdict>, String>;

/// What git said of each worktree's commits, kept until they differ, the
/// rules of each project as last read, and each worktree's judgment, kept
/// until its tip or the rules differ.
pub struct Reads {
    /// The rules checkout, or why there is none (HOME is not set).
    root: Result<PathBuf, String>,
    commits: Mutex<HashMap<ReadKey, Read>>,
    rules: Mutex<HashMap<String, Rules>>,
    /// How many times the rules files changed (`rules_changed`).
    rules_changes: AtomicU64,
    judged: Mutex<HashMap<PathBuf, HashMap<String, Judged>>>,
    /// Only one reader judges at a time. Completed reads never take this lock.
    judging: Mutex<()>,
}

impl Default for Reads {
    fn default() -> Reads {
        Reads::with_root(crate::review::default_rules_root().map_err(Fail::text))
    }
}

impl Reads {
    /// Reads that take the rules from the checkout at <root>, or that say
    /// why there is none.
    pub fn with_root(root: Result<PathBuf, String>) -> Reads {
        Reads {
            root,
            commits: Mutex::new(HashMap::new()),
            rules: Mutex::new(HashMap::new()),
            rules_changes: AtomicU64::new(0),
            judged: Mutex::new(HashMap::new()),
            judging: Mutex::new(()),
        }
    }

    /// The paths each of <shas> changes in <top>, or why git could not read
    /// them. Reads keeps only a read that succeeded, so the next page build
    /// asks git again after a failure.
    fn commits(&self, top: &Path, shas: &[String]) -> Result<Read, String> {
        let key = (top.to_path_buf(), shas.to_vec());
        let mut known = self.commits.lock().expect("no panic holding it");
        if let Some(found) = known.get(&key) {
            return Ok(found.clone());
        }
        let read = commits::paths_of_each(top, shas).map_err(|why| {
            format!(
                "reading the paths the commits of {} change: {}",
                top.display(),
                why.text()
            )
        })?;
        if known.len() > 256 {
            known.clear();
        }
        known.insert(key, read.clone());
        Ok(read)
    }

    /// The folders of the rules checkout whose files `Material::load`
    /// reads; none when there is no rules checkout. Whoever follows them
    /// calls `rules_changed` after each change under them.
    pub fn rules_folders(&self) -> Vec<PathBuf> {
        match &self.root {
            Ok(root) => Material::folders(root),
            Err(_) => Vec::new(),
        }
    }

    /// Files under `rules_folders` changed: the next ask reads the rules
    /// again.
    pub fn rules_changed(&self) {
        self.rules_changes.fetch_add(1, Ordering::SeqCst);
    }

    /// <project>'s rules, read from the rules checkout once and again after
    /// each change to its files, or why they can't be read.
    fn rules(&self, project: &str) -> Result<Arc<Material>, String> {
        let root = self.root.as_ref().map_err(Clone::clone)?;
        let mut rules = self.rules.lock().expect("no panic holding it");
        // Taken before the read: a change during the read moves it, and the
        // next ask reads again.
        let changes = self.rules_changes.load(Ordering::SeqCst);
        let held = rules.get(project).cloned();
        if let Some((read_after, material)) = &held
            && *read_after == changes
        {
            return material.clone();
        }
        let material = Material::load(root, project).map(Arc::new).map_err(|why| {
            format!(
                "reading the rules of {project} from {}: {}",
                root.display(),
                why.text()
            )
        });
        rules.insert(project.to_string(), (changes, material.clone()));
        material
    }

    fn kept(&self, project: &str, top: &Path, tip: &Tip, rules_version: u64) -> Option<Outcome> {
        let judged = self.judged.lock().expect("no panic holding it");
        let kept = judged.get(top)?.get(project)?;
        if &kept.tip == tip && kept.rules_version == rules_version {
            return Some(kept.verdict.clone());
        }
        None
    }

    /// The verdict for the current inputs if a read finished. Never waits
    /// for Git or a store.
    pub fn completed(&self, project: &str, version: u64, worktree: &Worktree) -> Option<Outcome> {
        let rules_version = self.rules_changes.load(Ordering::SeqCst);
        self.kept(
            project,
            worktree.top,
            &Tip::of(worktree, version),
            rules_version,
        )
    }

    /// Judges a pending dashboard tip. Returns true if it published a verdict.
    pub fn fill(&self, store: &Store, version: u64, worktree: &Worktree) -> bool {
        let (outcome, changed) = self.read_at(store, version, worktree, false);
        if changed && let Err(error) = outcome {
            crate::text::note(format!(
                "reading the rules review of {} in {}: {error}",
                worktree.top.display(),
                store.project
            ));
        }
        changed
    }

    fn read_at(
        &self,
        store: &Store,
        version: u64,
        worktree: &Worktree,
        retry_failed: bool,
    ) -> (Outcome, bool) {
        let _judging = self.judging.lock().expect("no panic holding it");
        let tip = Tip::of(worktree, version);
        let rules_version = self.rules_changes.load(Ordering::SeqCst);
        if let Some(kept) = self.kept(&store.project, worktree.top, &tip, rules_version)
            && (!retry_failed || kept.is_ok())
        {
            return (kept, false);
        }
        let verdict = self
            .rules(&store.project)
            .and_then(|rules| self.judge(store, worktree, &rules))
            .map(Arc::new);
        self.judged
            .lock()
            .expect("no panic holding it")
            .entry(worktree.top.to_path_buf())
            .or_default()
            .insert(
                store.project.clone(),
                Judged {
                    tip,
                    rules_version,
                    verdict: verdict.clone(),
                },
            );
        (verdict, true)
    }

    /// The verdict on the commits of <worktree>, a worktree of <lists>'
    /// project, whose store is at the version <lists> says.
    pub fn verdict_of(
        &self,
        places: &Places,
        lists: &Lists,
        worktree: &Worktree,
    ) -> Result<Arc<Verdict>, String> {
        let store = Store::named(places, &lists.project).map_err(Fail::text)?;
        self.verdict_at(&store, lists.version, worktree)
    }

    /// The verdict on the commits of <worktree>, a worktree of <store>'s
    /// project, with the store at <version>.
    pub fn verdict_at(
        &self,
        store: &Store,
        version: u64,
        worktree: &Worktree,
    ) -> Result<Arc<Verdict>, String> {
        self.read_at(store, version, worktree, true).0
    }

    /// Judges <worktree>'s commits the way `builds land` does, against the
    /// groups and rules of <material>.
    fn judge(
        &self,
        store: &Store,
        worktree: &Worktree,
        material: &Material,
    ) -> Result<Verdict, String> {
        let shas = worktree.git.ahead_oldest_first();
        let paths = self.commits(worktree.top, &shas)?;
        let conn = store.open_existing().map_err(Fail::text)?;
        // The store's version is part of the tip a verdict is kept for, so a
        // change to the repo's design globs judges the worktree again.
        let design = Globs::read(&conn, worktree.repo)
            .and_then(|files| files.with_capture_folders(worktree.top))
            .map_err(Fail::text)?;
        let scope = Scope {
            material,
            files: &design,
        };
        let needs: Vec<Need> = shas
            .iter()
            .zip(&paths)
            .map(|(sha, paths)| Need {
                sha: sha.clone(),
                paths: paths.clone(),
                groups: scope.group_names(paths),
            })
            .collect();
        let key = crate::review::default_key(worktree.top).map_err(Fail::text)?;
        let change = Change {
            repo: worktree.repo,
            key: &key,
        };
        landing::judge(
            &conn,
            change,
            worktree.top,
            &needs,
            CodeCheck::Branch("HEAD"),
            scope,
        )
        .map_err(Fail::text)
    }
}
