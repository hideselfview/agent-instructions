//! The verdict of the review on a worktree's commits, judged the way
//! `builds land` judges them (`landing::judge`), and kept so that the
//! dashboard's cards and health's findings read one judgment.
//!
//! Judging asks git for the branch's net change, so `Reads` judges a worktree
//! once for each tip: its branch, HEAD and origin/main's commit, the version
//! of its project's store, and the rules. It judges again only when one of
//! those differs.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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
        if let Some((read_after, Ok(material))) = &held
            && *read_after == changes
        {
            return Ok(material.clone());
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
        let rules = self.rules(&store.project)?;
        self.verdict(store, worktree, Tip::of(worktree, version), rules)
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

type Outcome = Result<Arc<Verdict>, String>;
type Place = (PathBuf, String);

#[derive(Clone, PartialEq, Eq)]
struct QueryTip {
    git_and_store: Tip,
    rules: u64,
}

struct Query {
    place: Place,
    tip: QueryTip,
    store: Store,
    repo: String,
    git: SideRead,
}

impl Query {
    fn read(&self, reads: &Reads) -> Outcome {
        let worktree = Worktree {
            top: &self.place.0,
            repo: &self.repo,
            branch: self.tip.git_and_store.branch.as_deref(),
            git: &self.git,
        };
        reads.verdict_at(&self.store, self.tip.git_and_store.store, &worktree)
    }
}

enum QueryState {
    Pending { previous_error: Option<String> },
    Read { outcome: Outcome, at: Instant },
}

struct Entry {
    tip: QueryTip,
    state: QueryState,
}

#[derive(Default)]
struct Work {
    entries: HashMap<Place, Entry>,
    queue: VecDeque<Query>,
    stopped: bool,
    failure: Option<String>,
}

/// Dashboard requests for review verdicts. Only the worker writes completed
/// answers here. A synchronous health read can reuse the Git and review
/// reads without changing the dashboard's answers.
#[derive(Default)]
pub struct Queries {
    work: Mutex<Work>,
    ready: Condvar,
}

/// A card keeps showing a failed read during backoff. Its next request after
/// the backoff can ask for another read of the same inputs.
const REVIEW_RETRY: Duration = Duration::from_secs(2);

impl Queries {
    /// Requests a verdict without waiting for Git. None means the worker
    /// has the request and has yet to give an answer.
    pub fn request(
        &self,
        reads: &Reads,
        store: &Store,
        version: u64,
        worktree: &Worktree,
    ) -> Option<Outcome> {
        let place = (worktree.top.to_path_buf(), store.project.clone());
        let tip = QueryTip {
            git_and_store: Tip::of(worktree, version),
            rules: reads.rules_changes.load(Ordering::SeqCst),
        };
        let mut work = self.work.lock().expect("no panic holding it");
        if let Some(failure) = &work.failure {
            return Some(Err(failure.clone()));
        }
        let mut previous_error = None;
        if let Some(entry) = work.entries.get(&place).filter(|entry| entry.tip == tip) {
            match &entry.state {
                QueryState::Pending { previous_error } => {
                    return previous_error.clone().map(Err);
                }
                QueryState::Read { outcome, at } => match outcome {
                    Err(error) if at.elapsed() >= REVIEW_RETRY => {
                        previous_error = Some(error.clone());
                    }
                    _ => return Some(outcome.clone()),
                },
            }
        }
        work.entries.insert(
            place.clone(),
            Entry {
                tip: tip.clone(),
                state: QueryState::Pending {
                    previous_error: previous_error.clone(),
                },
            },
        );
        work.queue.retain(|query| query.place != place);
        work.queue.push_back(Query {
            place,
            tip,
            store: store.clone(),
            repo: worktree.repo.to_string(),
            git: worktree.git.clone(),
        });
        self.ready.notify_one();
        previous_error.map(Err)
    }

    fn next(&self) -> Option<Query> {
        let work = self.work.lock().expect("no panic holding it");
        let mut work = self
            .ready
            .wait_while(work, |work| work.queue.is_empty() && !work.stopped)
            .expect("no panic holding it");
        if work.stopped {
            return None;
        }
        work.queue.pop_front()
    }

    fn answer(&self, query: &Query, outcome: Outcome) -> bool {
        let mut work = self.work.lock().expect("no panic holding it");
        let Some(entry) = work
            .entries
            .get_mut(&query.place)
            .filter(|entry| entry.tip == query.tip)
        else {
            return false;
        };
        let QueryState::Pending { previous_error } = &entry.state else {
            return false;
        };
        let changed = outcome.is_ok() || outcome.as_ref().err() != previous_error.as_ref();
        let error = outcome.as_ref().err().cloned();
        entry.state = QueryState::Read {
            outcome,
            at: Instant::now(),
        };
        drop(work);
        if changed && let Some(error) = error {
            crate::text::note(format!(
                "reading the rules review of {} in {}: {error}",
                query.place.0.display(),
                query.store.project,
            ));
        }
        changed
    }

    /// Reads queued verdicts and tells the page when it has a new answer.
    /// A panic ends the worker and makes the failure visible to its readers.
    pub fn run(&self, reads: &Reads, publish: impl Fn()) {
        let worker = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Some(query) = self.next() {
                if self.answer(&query, query.read(reads)) {
                    publish();
                }
            }
        }));
        if worker.is_err() {
            let failure = "The dashboard's rules review worker stopped after a panic.";
            crate::text::note(failure);
            self.work.lock().expect("no panic holding it").failure = Some(failure.to_string());
            publish();
        }
    }

    #[cfg(test)]
    pub fn stop(&self) {
        self.work.lock().expect("no panic holding it").stopped = true;
        self.ready.notify_one();
    }
}
