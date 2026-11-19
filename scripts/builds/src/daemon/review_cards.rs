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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::cards::{ItemWait, ReviewWait, Settler};
use crate::fail::Fail;
use crate::lists::store::Store;
use crate::review::commits;
use crate::review::landing::{self, CodeCheck, Need, Verdict};
use crate::review::record::Change;
use crate::review::run::Material;
use crate::side_git::SideGit;

/// How long the rules read from the rules checkout are used before they are
/// read again.
const RULES_FOR: Duration = Duration::from_secs(10);

/// A worktree and its commits (full hashes, oldest first).
type ReadKey = (PathBuf, Vec<String>);

/// The paths each commit of a worktree changes.
type Read = Vec<Vec<String>>;

/// A project's rules as `Material::load` last read them, or why it failed,
/// and when. A read that finds the same rules keeps the earlier `Arc`, so a
/// worktree judged against them need not be judged again.
type Rules = (Instant, Result<Arc<Material>, String>);

/// A side worktree whose card shows where its rules review stands, as the
/// daemon last read it.
pub struct Worktree<'a> {
    pub repo: &'a str,
    pub top: &'a Path,
    /// The branch HEAD is on; None when HEAD is detached.
    pub branch: Option<&'a str>,
    pub git: &'a SideGit,
}

/// What a worktree's standing is judged from besides the rules. HEAD and
/// origin/main's commit fix the commits waiting to land and the branch's net
/// change; the branch names the review's change; the store's version moves
/// with every write to the review's records.
#[derive(Clone, PartialEq, Eq)]
struct Tip {
    branch: Option<String>,
    head: Option<String>,
    main: Option<String>,
    store: u64,
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

/// A worktree's judgment and what it was judged from.
struct Judged {
    tip: Tip,
    rules: Arc<Material>,
    judgment: Judgment,
}

/// What git said of each worktree's commits, kept until they differ, the
/// rules of each project as last read, and each worktree's judgment, kept
/// until its tip or the rules differ.
pub struct Reads {
    /// The rules checkout, or why there is none (HOME is not set).
    root: Result<PathBuf, String>,
    commits: Mutex<HashMap<ReadKey, Read>>,
    rules: Mutex<HashMap<String, Rules>>,
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

    /// <project>'s rules, read from the rules checkout at most every
    /// `RULES_FOR`, or why they can't be read.
    fn rules(&self, project: &str) -> Result<Arc<Material>, String> {
        let root = self.root.as_ref().map_err(Clone::clone)?;
        let mut rules = self.rules.lock().expect("no panic holding it");
        let held = rules.get(project).cloned();
        if let Some((at, material)) = &held
            && at.elapsed() < RULES_FOR
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
        rules.insert(project.to_string(), (Instant::now(), material.clone()));
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
        let tip = Tip {
            branch: worktree.branch.map(str::to_string),
            head: worktree.git.head.as_ref().map(|head| head.full.clone()),
            main: worktree.git.main.clone(),
            store: version,
        };
        let wait = match self.judgment(store, worktree, tip, rules)? {
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

    /// The judgment of <worktree>'s commits at <tip> against <rules>: the
    /// one kept for the worktree when it was judged at the same tip against
    /// the same rules, else judged now. Reads keeps only a judgment that
    /// succeeded, so the next page build judges again after a failure.
    fn judgment(
        &self,
        store: &Store,
        worktree: &Worktree,
        tip: Tip,
        rules: Arc<Material>,
    ) -> Result<Judgment, String> {
        // Held while judging, so builds that ask at once judge once.
        let mut judged = self.judged.lock().expect("no panic holding it");
        if let Some(kept) = judged.get(worktree.top)
            && kept.tip == tip
            && Arc::ptr_eq(&kept.rules, &rules)
        {
            return Ok(kept.judgment);
        }
        let judgment = self.judge(store, worktree, &rules)?;
        judged.insert(
            worktree.top.to_path_buf(),
            Judged {
                tip,
                rules,
                judgment,
            },
        );
        Ok(judgment)
    }

    /// Judges <worktree>'s commits the way `builds land` does, against the
    /// groups and rules of <material>.
    fn judge(
        &self,
        store: &Store,
        worktree: &Worktree,
        material: &Material,
    ) -> Result<Judgment, String> {
        let shas: Vec<String> = worktree
            .git
            .ahead
            .iter()
            .rev()
            .map(|c| c.full.clone())
            .collect();
        let paths = self.commits(worktree.top, &shas)?;
        let needs: Vec<Need> = shas
            .iter()
            .zip(&paths)
            .map(|(sha, paths)| Need {
                sha: sha.clone(),
                paths: paths.clone(),
                groups: material.group_names(paths),
            })
            .collect();
        if needs.iter().all(|n| n.groups.is_empty()) {
            return Ok(Judgment::NotNeeded);
        }
        let conn = store.open_existing().map_err(Fail::text)?;
        let key = crate::review::default_key(worktree.top).map_err(Fail::text)?;
        let change = Change {
            repo: worktree.repo,
            key: &key,
        };
        let check = CodeCheck::Branch("HEAD");
        Ok(
            match landing::judge(&conn, change, worktree.top, &needs, check, material)
                .map_err(Fail::text)?
            {
                Verdict::NotNeeded => Judgment::NotNeeded,
                Verdict::Takes(_) => Judgment::Takes,
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
            },
        )
    }
}

#[cfg(test)]
#[path = "review_cards_tests.rs"]
mod tests;
