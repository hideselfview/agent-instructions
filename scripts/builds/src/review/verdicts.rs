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
use crate::review::commits::{self, Unread};
use crate::review::landing::{self, CodeCheck, Need, Verdict};
use crate::review::record::Change;
use crate::review::run::{Material, Scope};
use crate::side_git::SideRead;

/// A worktree, its commits (full hashes, oldest first) and the folders of its
/// repo that no review reads.
type ReadKey = (PathBuf, Vec<String>, Unread);

/// The paths each commit of a worktree changes.
type Read = Vec<commits::CommitPaths>;

/// A project's last valid rules and the version of the whole rules checkout
/// they came from. A new read with the same review rules keeps the earlier
/// `Arc`, so worktrees judged against them need not be judged again.
type Rules = (u64, Arc<Material>);

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
    root: PathBuf,
    commits: Mutex<HashMap<ReadKey, Read>>,
    rules: Mutex<HashMap<String, Rules>>,
    rules_version: AtomicU64,
    judged: Mutex<HashMap<PathBuf, Judged>>,
}

impl Reads {
    /// Reads that take the rules from the checkout at <root>.
    pub fn with_root(root: PathBuf) -> Reads {
        Reads {
            root,
            commits: Mutex::new(HashMap::new()),
            rules: Mutex::new(HashMap::new()),
            rules_version: AtomicU64::new(0),
            judged: Mutex::new(HashMap::new()),
        }
    }

    /// The paths each of <shas> changes in <top>, or why git could not read
    /// them. Reads keeps only a read that succeeded, so the next page build
    /// asks git again after a failure.
    fn commits(&self, top: &Path, shas: &[String], unread: &Unread) -> Result<Read, String> {
        let key = (top.to_path_buf(), shas.to_vec(), unread.clone());
        let mut known = self.commits.lock().expect("no panic holding it");
        if let Some(found) = known.get(&key) {
            return Ok(found.clone());
        }
        let read = commits::paths_of_each(top, shas, unread).map_err(|why| {
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

    /// The shared rules and project folders that `Material::load` reads.
    pub fn rules_folders(&self) -> [PathBuf; 2] {
        Material::folders(&self.root)
    }

    /// A change under either rules folder can invalidate the entire
    /// checkout, since `Material::load` validates every project's rules.
    pub fn rules_changed(&self) {
        self.rules_version.fetch_add(1, Ordering::Relaxed);
    }

    fn rules_version(&self) -> u64 {
        self.rules_version.load(Ordering::Relaxed)
    }

    /// <project>'s rules, read from the rules checkout once and again after
    /// each change to that checkout, or why they can't be read.
    fn rules(&self, project: &str) -> Result<Arc<Material>, String> {
        let root = &self.root;
        let mut rules = self.rules.lock().expect("no panic holding it");
        // Taken before the read: a change during the read moves it, and the
        // next ask reads again.
        let changes = self.rules_version();
        let held = rules.get(project);
        if let Some((read_after, material)) = held
            && *read_after == changes
        {
            return Ok(material.clone());
        }
        let read = Material::load(root, project).map_err(|why| {
            format!(
                "reading the rules of {project} from {}: {}",
                root.display(),
                why.text()
            )
        })?;
        let material = match held {
            Some((_, before)) if same_review_rules(before, &read) => before.clone(),
            _ => Arc::new(read),
        };
        rules.insert(project.to_string(), (changes, material.clone()));
        Ok(material)
    }

    /// The verdict on <worktree>'s commits at <tip> against <rules>: the
    /// one kept for the worktree when it was judged at the same tip against
    /// the same rules, else judged now. A failed read leaves no verdict for
    /// later callers.
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
        let unread = material.unread(worktree.repo);
        let paths = self.commits(worktree.top, &shas, &unread)?;
        let conn = store.open_existing().map_err(Fail::text)?;
        // The store's version is part of the tip a verdict is kept for, so a
        // change to the repo's design globs judges the worktree again.
        let design = Globs::read(&conn, worktree.repo)
            .and_then(|files| files.with_capture_folders(worktree.top))
            .map_err(Fail::text)?;
        let scope = Scope {
            material,
            files: &design,
            unread: &unread,
        };
        let needs: Vec<Need> = shas
            .iter()
            .zip(&paths)
            .map(|(sha, changed)| Need::of(sha.clone(), changed, scope))
            .collect();
        let key =
            crate::review::landing_key(&conn, worktree.repo, worktree.top).map_err(Fail::text)?;
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

/// Only groups and rules that affect the verdict count here. Reviewer
/// prompts and rules that belong to other projects do not affect it.
fn same_review_rules(before: &Material, after: &Material) -> bool {
    fn groups(
        material: &Material,
    ) -> impl Iterator<Item = (&str, bool, Vec<&crate::review::rules::Rule>)> {
        material
            .groups
            .reviewing(&material.rules)
            .filter(|(_, members)| !members.is_empty())
            .map(|(group, members)| (group.name.as_str(), group.commit_text, members))
    }
    before.unread_folders == after.unread_folders && groups(before).eq(groups(after))
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
    Ready(Arc<Verdict>),
    Failed { error: String, at: Instant },
}

struct Entry {
    tip: QueryTip,
    state: QueryState,
}

#[derive(Default)]
struct Work {
    entries: HashMap<Place, Entry>,
    queue: VecDeque<Query>,
    #[cfg(test)]
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

/// The dashboard's rules review worker. The active request stays here
/// through unwinding so the thread boundary can report what failed.
pub struct Worker<'a> {
    queries: &'a Queries,
    active: Option<Query>,
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
            rules: reads.rules_version(),
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
                QueryState::Ready(verdict) => return Some(Ok(verdict.clone())),
                QueryState::Failed { error, at } if at.elapsed() < REVIEW_RETRY => {
                    return Some(Err(error.clone()));
                }
                QueryState::Failed { error, .. } => previous_error = Some(error.clone()),
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
            .wait_while(work, |work| {
                #[cfg(test)]
                if work.stopped {
                    return false;
                }
                work.queue.is_empty()
            })
            .expect("no panic holding it");
        #[cfg(test)]
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
        entry.state = match outcome {
            Ok(verdict) => QueryState::Ready(verdict),
            Err(error) => QueryState::Failed {
                error,
                at: Instant::now(),
            },
        };
        drop(work);
        if changed && let Some(error) = error {
            crate::output::note(format!(
                "reading the rules review of {} in {}: {error}",
                query.place.0.display(),
                query.store.project,
            ));
        }
        changed
    }

    pub fn worker(&self) -> Worker<'_> {
        Worker {
            queries: self,
            active: None,
        }
    }

    #[cfg(test)]
    pub fn stop(&self) {
        self.work.lock().expect("no panic holding it").stopped = true;
        self.ready.notify_one();
    }
}

impl Worker<'_> {
    /// Reads queued verdicts and tells the page when it has a new answer.
    pub fn run(&mut self, reads: &Reads, publish: impl Fn()) {
        while let Some(query) = self.queries.next() {
            let query = self.active.insert(query);
            if self.queries.answer(query, query.read(reads)) {
                publish();
            }
            self.active = None;
        }
    }

    /// Reports the caught panic and its operation in the daemon log, and
    /// answers present and future requests with that failure.
    pub fn report_panic(self, panic: Box<dyn std::any::Any + Send>) {
        let detail = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("Rust supplied a non-string panic payload");
        let operation = match self.active {
            Some(query) => format!(
                "working on {} in project {}",
                query.place.0.display(),
                query.store.project
            ),
            None => "waiting for a request".to_string(),
        };
        let failure = format!(
            "The dashboard's rules review worker stopped while {operation}. Rust reported {detail}."
        );
        crate::output::note(&failure);
        self.queries
            .work
            .lock()
            .expect("no panic holding it")
            .failure = Some(failure);
    }
}
