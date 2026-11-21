//! Where the rules review of a worktree's commits stands, for its card: the
//! commits waiting to land are judged the way `builds land` judges them
//! (`review::landing`), so a card says Ready to land only when land would take
//! them. A review that runs now over the branch's newest commit shows as
//! running.
//!
//! Every open page and the recorder of the cards' states ask for each card's
//! standing each time they build it. Judging asks git for the branch's net
//! change, so `Reads` judges a worktree once for each tip: its branch, HEAD
//! and origin/main's commit, the version of its project's store, and the
//! rules. It judges again only when one of those differs. Whether a review
//! runs now, and whether the judge's session runs, change with no write to
//! the store, so `standing` reads them each time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::cards::{ItemWait, ReviewWait, Settler};
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
/// that finds the same rules keeps the earlier `Arc`, so a worktree judged
/// against them need not be judged again.
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

/// What the review's records and the branch's net change say of a
/// worktree's commits.
#[derive(Clone, Copy)]
enum Judgment {
    /// No group's rules cover a path the commits change.
    NotNeeded,
    /// Land takes the commits as far as the review goes.
    Takes,
    /// No review of the change is open, or a group gave no answer over a
    /// diff the review read.
    NotRun,
    /// This many findings wait for a settle.
    ToSettle(usize),
    /// This many paths, or the commit messages, changed since the last review.
    Unreviewed(usize),
    /// This many confirmed findings are open.
    Open(usize),
}

impl Judgment {
    /// What <verdict> says of the commits, as a card shows it.
    fn of(verdict: &Verdict) -> Judgment {
        match verdict {
            Verdict::NotNeeded => Judgment::NotNeeded,
            Verdict::Takes(_) | Verdict::OverCap(_) => Judgment::Takes,
            Verdict::NoRun => Judgment::NotRun,
            Verdict::Unfinished(_, unfinished) if !unfinished.unsettled.is_empty() => {
                Judgment::ToSettle(unfinished.unsettled.len())
            }
            Verdict::Unfinished(_, unfinished) if !unfinished.unanswered.is_empty() => {
                Judgment::NotRun
            }
            Verdict::Unfinished(_, unfinished) if !unfinished.unreviewed.is_empty() => {
                Judgment::Unreviewed(unfinished.unreviewed.len())
            }
            Verdict::Unfinished(_, unfinished) => Judgment::Open(unfinished.open.len()),
        }
    }
}

/// A worktree's verdict and what it was judged from.
struct Judged {
    tip: Tip,
    rules: Arc<Material>,
    verdict: Arc<Verdict>,
}

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
    judged: Mutex<HashMap<PathBuf, Judged>>,
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
        let material = match (
            held.and_then(|(_, material)| material.ok()),
            Material::load(root, project),
        ) {
            (Some(before), Ok(read)) if *before == read => Ok(before),
            (_, read) => read.map(Arc::new).map_err(|why| {
                format!(
                    "reading the rules of {project} from {}: {}",
                    root.display(),
                    why.text()
                )
            }),
        };
        rules.insert(project.to_string(), (changes, material.clone()));
        material
    }

    /// Where the review of <worktree>'s commits stands, for a worktree
    /// whose project's store is <store>, at version <version> as the daemon
    /// last read it; None when land would take them as far as the review
    /// goes, or when they need no review. Findings to settle wait on the
    /// judge while one listens, which Claude Code's running sessions in
    /// <sessions> say. Fails, saying why, when git cannot read the commits
    /// or builds cannot open or read the store or the sessions.
    pub fn standing(
        &self,
        store: &Store,
        version: u64,
        sessions: &Path,
        worktree: &Worktree,
    ) -> Result<Option<ItemWait>, String> {
        let Some(newest) = worktree.git.ahead.first() else {
            return Ok(None);
        };
        let rules = self.rules(&store.project)?;
        let tip = Tip::of(worktree, version);
        let verdict = self.verdict(store, worktree, tip, rules)?;
        let wait = match Judgment::of(&verdict) {
            Judgment::NotNeeded => return Ok(None),
            _ if crate::review::is_running(&store.dir, &newest.full) => ReviewWait::Running,
            Judgment::Takes => return Ok(None),
            Judgment::NotRun => ReviewWait::NotRun,
            Judgment::ToSettle(count) => {
                let conn = store.open_existing().map_err(Fail::text)?;
                let settler =
                    match crate::review::judge::listening(&conn, sessions).map_err(Fail::text)? {
                        Some(_) => Settler::Judge,
                        None => Settler::Coordinator,
                    };
                ReviewWait::ToSettle(count, settler)
            }
            Judgment::Unreviewed(count) => ReviewWait::Unreviewed(count),
            Judgment::Open(count) => ReviewWait::Open(count),
        };
        Ok(Some(ItemWait::RulesReview(wait)))
    }

    /// The verdict on <worktree>'s commits at <tip> against <rules>: the
    /// one kept for the worktree when it was judged at the same tip against
    /// the same rules, else judged now. Reads keeps only a verdict that
    /// succeeded, so the next page build judges again after a failure.
    fn verdict(
        &self,
        store: &Store,
        worktree: &Worktree,
        tip: Tip,
        rules: Arc<Material>,
    ) -> Result<Arc<Verdict>, String> {
        // Held while judging, so builds that ask at once judge once.
        let mut judged = self.judged.lock().expect("no panic holding it");
        if let Some(kept) = judged.get(worktree.top)
            && kept.tip == tip
            && Arc::ptr_eq(&kept.rules, &rules)
        {
            return Ok(kept.verdict.clone());
        }
        let verdict = Arc::new(self.judge(store, worktree, &rules)?);
        judged.insert(
            worktree.top.to_path_buf(),
            Judged {
                tip,
                rules,
                verdict: verdict.clone(),
            },
        );
        Ok(verdict)
    }

    /// The verdict on the commits of <worktree>, a worktree of <lists>'
    /// project, at the tip and rules `standing` keeps it for.
    pub fn verdict_of(
        &self,
        places: &Places,
        lists: &Lists,
        worktree: &Worktree,
    ) -> Result<Arc<Verdict>, String> {
        let store = Store::named(places, &lists.project).map_err(Fail::text)?;
        let rules = self.rules(&lists.project)?;
        self.verdict(&store, worktree, Tip::of(worktree, lists.version), rules)
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

#[cfg(test)]
#[path = "review_cards_tests.rs"]
mod tests;
