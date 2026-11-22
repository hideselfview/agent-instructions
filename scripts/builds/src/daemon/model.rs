//! The daemon's live picture of every checkout, kept current from events:
//! the volume's (sizes, roots, app checkouts), the checkouts' own (git state,
//! claims, manifests), and, while a page watches, the processes'. Each part
//! is read again only when something it depends on changed, and every change
//! moves `version`, which the page's streams wait on.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::clock::Moment;

use serde::Serialize;
use serde_json::{Value, json};

use super::cards::{CardState, ItemWait, KeptCard, ReviewWait, RunningItem};
use super::fsevents::{Stream, flags};
use super::lists::{Feed, compose, running_worktrees};
use super::runs::{RunPage, Runs};
use crate::checkout::{Checkout, Root, absolute_link_target, all_roots, parallel_map};
use crate::claim::Claim;
use crate::health::{
    Claimed, Health, Machine, Unclaimed, assess, locks_of, places_of, sample, sccache_reading,
};
use crate::lists::design_paths::Globs;
use crate::lists::view::{ClaimFacts, RulesReview};
use crate::manifest::{BuildDir, build_dirs};
use crate::places::{DiskSpace, Places, Rules};
use crate::procs::Place;
use crate::side_git::SideGit;
use crate::tool::volume_space;
use crate::usage::Measure;

/// Changes wait this long for more before they are read: agents write
/// files in bursts.
pub(super) const SETTLE: Duration = Duration::from_millis(300);
/// How often the processes are read while a page watches.
const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// How often the processes are read while no page watches, for the commands
/// that ask for health.
const IDLE_SAMPLE_EVERY: Duration = Duration::from_secs(10);
/// How often the hosts are asked while a page watches, and while none does.
const HOSTS_EVERY: Duration = Duration::from_secs(5);
const IDLE_HOSTS_EVERY: Duration = Duration::from_secs(60);
/// How often sccache is asked for its count.
const SCCACHE_EVERY: Duration = Duration::from_secs(30);
/// How long a host's load samples are kept for the page's line, seconds.
const HOST_LOAD_SECONDS: i64 = 600;
/// Load samples kept for the page's line: ten minutes at SAMPLE_EVERY.
const LOAD_KEPT: usize = 300;

/// What needs reading again.
#[derive(Default)]
struct Dirty {
    layout: bool,
    /// Build roots whose sizes changed.
    sizes: HashSet<PathBuf>,
    /// Checkout tops whose refs, claim or manifest changed: git is asked
    /// everything about them again.
    git: HashSet<PathBuf>,
    /// Side checkout tops where only files of the working tree changed: git
    /// is asked for the uncommitted files alone.
    tree: HashSet<PathBuf>,
}

impl Dirty {
    fn any(&self) -> bool {
        self.layout || !self.sizes.is_empty() || !self.git.is_empty() || !self.tree.is_empty()
    }
}

/// What a checkout's folders and git say.
#[derive(Clone, Default)]
struct Facts {
    manifest: Vec<BuildDir>,
    claim: Option<Claim>,
    branch: Option<String>,
    git: Option<SideGit>,
    /// Per manifest entry, in order; None until the daemon reads the sizes,
    /// which it does after the checkout's git, so a page asked for between
    /// the two shows the size as not known yet, not as nothing.
    folders: Option<Vec<FolderFacts>>,
    /// A gone root's whole size.
    gone_kib: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct FolderFacts {
    pub path: String,
    pub cap_kib: u64,
    pub exists: bool,
    pub kib: u64,
    pub last_write: Option<i64>,
    pub link: &'static str,
}

/// What the daemon learned the last time it listed the processes: what they
/// do, the list of places that each build's place index points into, and
/// the volume's free space. Each time the daemon works out health from it,
/// the daemon reads every claim from its file at that moment.
#[derive(Clone)]
struct Sample {
    machine: Machine,
    places: Vec<Place>,
    disk: DiskSpace,
}

#[derive(Default)]
struct State {
    version: u64,
    roots: Vec<Root>,
    facts: HashMap<PathBuf, Facts>,
    sample: Option<Sample>,
    /// pid -> (CPU seconds, when read), for CPU %.
    cpu: HashMap<i32, (f64, Moment)>,
    cpu_percent: HashMap<i32, f64>,
    load: VecDeque<(i64, f64)>,
    health: Option<Health>,
    sccache_at: Option<Instant>,
    sccache: Option<crate::health::SccacheServer>,
    /// The other hosts' load cards (hosts.rs), empty before the first reading.
    hosts: Vec<super::hosts::HostPage>,
    /// Each host's hit rate of Rust compiles over the last hour, day and
    /// week (sccache_history.rs), empty before the first reading.
    compile_cache: Vec<crate::sccache_history::HostRates>,
    /// Why the history could not be read, when it could not.
    compile_cache_error: Option<String>,
    /// What the hosts' reading found that `assess` does not (busy processes,
    /// hosts that refuse jobs: `health::host_findings`), which the page shows
    /// after the health's own findings.
    host_findings: Vec<crate::health::Finding>,
    /// Each host's 1-minute load over the last ten minutes, by host name.
    host_load: HashMap<String, VecDeque<(i64, f64)>>,
    /// What the daemon last read of each repo's branches, by the repo's main
    /// checkout; the daemon reads them again when any checkout of the repo
    /// changes.
    loose: HashMap<PathBuf, BranchReading>,
}

/// What the daemon last read of a repo's branches.
enum BranchReading {
    /// The branches with commits waiting to land that no worktree checks
    /// out, and what the reading found of each branch tip, for the next
    /// reading to start from.
    Read(crate::unlanded::Judged, Vec<crate::unlanded::LooseBranch>),
    /// Why the reading failed.
    Failed(String),
}

pub struct Model {
    pub places: Places,
    rules: Rules,
    state: Mutex<State>,
    changed: Condvar,
    dirty: Mutex<Dirty>,
    wake: Condvar,
    viewers: AtomicUsize,
    viewing: Condvar,
    viewing_lock: Mutex<()>,
    /// Held while a process reading runs, so `stop` waits for one in progress.
    sampling: Mutex<()>,
    /// The process reading ends at its next turn.
    stopped: AtomicBool,
    git_stream: Mutex<Option<(Vec<PathBuf>, Stream)>>,
    /// Follows the rules files the rules review reads, while it runs.
    rules_stream: Mutex<Option<Stream>>,
    lists: Arc<Feed>,
    /// Each worktree's design commits, which its card and health's findings
    /// both read.
    design_reads: crate::lists::design_reads::DesignReads,
    /// What git said of each branch's commits for its rules review.
    review_reads: crate::review::verdicts::Reads,
    /// The build-app runs the page starts.
    pub runs: Arc<Runs>,
    /// The lists JSON last built and what it was built from.
    served: Mutex<Option<(ServedKey, Arc<String>)>>,
}

/// What the lists JSON is built from besides the clock's second: the read
/// of the stores, the day, each running item's claim facts and the rules
/// review of each design change that waits on the person's approval.
type ServedKey = (
    u64,
    String,
    Vec<ClaimFacts>,
    Vec<(String, crate::lists::Id, RulesReview)>,
);

pub(super) fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs() as i64
}

impl Model {
    pub fn start(
        places: Places,
        rules: Rules,
        runs: Arc<Runs>,
        program: std::path::PathBuf,
    ) -> Arc<Model> {
        let model = Arc::new(Model {
            places: places.clone(),
            rules,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            dirty: Mutex::new(Dirty {
                layout: true,
                ..Dirty::default()
            }),
            wake: Condvar::new(),
            viewers: AtomicUsize::new(0),
            viewing: Condvar::new(),
            viewing_lock: Mutex::new(()),
            sampling: Mutex::new(()),
            stopped: AtomicBool::new(false),
            git_stream: Mutex::new(None),
            rules_stream: Mutex::new(None),
            design_reads: crate::lists::design_reads::DesignReads::with_program(&places, program),
            lists: Feed::start(places),
            review_reads: Default::default(),
            runs,
            served: Mutex::new(None),
        });
        let weak = Arc::downgrade(&model);
        model.lists.listen(Box::new(move || {
            if let Some(model) = weak.upgrade() {
                model.publish();
            }
        }));
        let weak = Arc::downgrade(&model);
        model.runs.listen(Box::new(move || {
            if let Some(model) = weak.upgrade() {
                model.publish();
            }
        }));
        model.follow_rules();
        let refresher = model.clone();
        std::thread::Builder::new()
            .name("builds-refresh".into())
            .spawn(move || refresher.refresh_loop())
            .expect("a thread starts");
        let sampler = model.clone();
        std::thread::Builder::new()
            .name("builds-sample".into())
            .spawn(move || sampler.sample_loop())
            .expect("a thread starts");
        let hosts = model.clone();
        std::thread::Builder::new()
            .name("builds-hosts".into())
            .spawn(move || hosts.hosts_loop())
            .expect("a thread starts");
        model
    }

    fn mark(&self, change: impl FnOnce(&mut Dirty)) {
        let mut dirty = self.dirty.lock().expect("no panic holding it");
        change(&mut dirty);
        self.wake.notify_all();
    }

    /// A command wrote <roots>, the build roots whose folders it measured
    /// (sizes.rs), or made or removed. A root the daemon does not list, or
    /// that no longer has its marker, changes the layout; the sizes of each
    /// are read again.
    pub fn wrote(&self, roots: &[PathBuf]) {
        let known: HashSet<PathBuf> = self
            .state
            .lock()
            .expect("no panic holding it")
            .roots
            .iter()
            .map(|root| root.path.clone())
            .collect();
        self.mark(|dirty| {
            for root in roots {
                if known.contains(root) != root.join(".checkout").is_file() {
                    dirty.layout = true;
                }
                dirty.sizes.insert(root.clone());
            }
        });
    }

    /// The stream following the checkouts lost events: every checkout's git
    /// state, claim and manifest, and every build root's size, may have
    /// changed.
    fn checkouts_lost(&self) {
        let roots = self
            .state
            .lock()
            .expect("no panic holding it")
            .roots
            .clone();
        self.mark(|dirty| {
            for root in &roots {
                if let Some(checkout) = &root.live {
                    dirty.git.insert(checkout.top.clone());
                }
                dirty.sizes.insert(root.path.clone());
            }
        });
    }

    /// The checkouts' own folders changed under <paths>.
    fn checkouts_changed(&self, paths: &[PathBuf]) {
        let roots = self
            .state
            .lock()
            .expect("no panic holding it")
            .roots
            .clone();
        self.mark(|dirty| {
            for path in paths {
                for root in &roots {
                    let Some(checkout) = &root.live else { continue };
                    let in_tree = path.starts_with(&checkout.top)
                        && !path.starts_with(checkout.main.join(".git"));
                    let in_own_git = !checkout.is_main() && path.starts_with(&checkout.git_dir);
                    let common = checkout.main.join(".git");
                    let in_common = path.starts_with(&common)
                        && !path.starts_with(common.join("objects"))
                        && !path.starts_with(common.join("worktrees"));
                    // Main's own working tree is the person's: only its
                    // branch shows, which moves in the common git dir.
                    if in_own_git || in_common {
                        dirty.git.insert(checkout.top.clone());
                    } else if in_tree && !checkout.is_main() {
                        dirty.tree.insert(checkout.top.clone());
                    }
                    if in_tree && path.parent().is_some_and(|p| p == checkout.top)
                        || *path == checkout.top
                    {
                        // A manifest change shows here too.
                        dirty.sizes.insert(root.path.clone());
                    }
                }
            }
        });
    }

    /// Follows the rules files the rules review reads: after each change
    /// under them, the cards' review standing reads the rules again. Started
    /// before any page asks for a card, so no change goes unseen.
    fn follow_rules(self: &Arc<Self>) {
        // FSEvents follows the folder a link names, not the link.
        let folders: Vec<PathBuf> = self
            .review_reads
            .rules_folders()
            .into_iter()
            .map(|folder| match std::fs::canonicalize(&folder) {
                Ok(real) => real,
                Err(error) => {
                    crate::text::note(format!(
                        "following the rules folder {} as named, since its real path can't be read: {error}; the cards read the rules again only when FSEvents reports a change there",
                        folder.display()
                    ));
                    folder
                }
            })
            .collect();
        if folders.is_empty() {
            return;
        }
        let weak = Arc::downgrade(self);
        let refs: Vec<&Path> = folders.iter().map(PathBuf::as_path).collect();
        let started = Stream::start(
            &refs,
            &[],
            0.2,
            Box::new(move |_| {
                if let Some(model) = weak.upgrade() {
                    model.review_reads.rules_changed();
                    model.publish();
                }
            }),
        );
        *self.rules_stream.lock().expect("no panic holding it") = Some(started);
    }

    /// Follows the folders holding every checkout, restarting the stream
    /// when that set changes.
    fn follow_checkouts(self: &Arc<Self>, roots: &[Root]) {
        let mut folders: Vec<PathBuf> = Vec::new();
        for root in roots {
            let Some(checkout) = &root.live else { continue };
            let folder = if checkout.is_main() {
                checkout.top.clone()
            } else {
                checkout.top.parent().unwrap_or(&checkout.top).to_path_buf()
            };
            if !folders.contains(&folder) {
                folders.push(folder);
            }
            if !folders.contains(&checkout.main) {
                folders.push(checkout.main.clone());
            }
        }
        folders.sort();
        let mut stream = self.git_stream.lock().expect("no panic holding it");
        if stream
            .as_ref()
            .is_some_and(|(watched, _)| *watched == folders)
        {
            return;
        }
        let weak = Arc::downgrade(self);
        let refs: Vec<&Path> = folders.iter().map(PathBuf::as_path).collect();
        let started = Stream::start(
            &refs,
            &[],
            0.2,
            Box::new(move |events| {
                if let Some(model) = weak.upgrade() {
                    // FSEvents drops events when its daemon falls behind;
                    // what changed is then unknown, so every checkout is read
                    // again.
                    if events.iter().any(|e| {
                        e.flags
                            & (flags::USER_DROPPED
                                | flags::KERNEL_DROPPED
                                | flags::MUST_SCAN_SUBDIRS
                                | flags::EVENT_IDS_WRAPPED)
                            != 0
                    }) {
                        model.checkouts_lost();
                    }
                    let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                    model.checkouts_changed(&paths);
                }
            }),
        );
        *stream = Some((folders, started));
    }

    fn refresh_loop(self: Arc<Self>) {
        loop {
            {
                let mut dirty = self.dirty.lock().expect("no panic holding it");
                while !dirty.any() {
                    dirty = self.wake.wait(dirty).expect("no panic holding it");
                }
            }
            // Debouncing: let a burst of writes finish.
            std::thread::sleep(SETTLE);
            let taken = std::mem::take(&mut *self.dirty.lock().expect("no panic holding it"));
            if let Err(error) = self.refresh(taken) {
                eprintln!(
                    "builds: reading the checkouts: {}",
                    match error {
                        crate::fail::Fail::Message(message) => message,
                        crate::fail::Fail::Exit(code) => format!("exit {code}"),
                    }
                );
            }
        }
    }

    fn refresh(self: &Arc<Self>, mut dirty: Dirty) -> crate::fail::Result<()> {
        if dirty.layout {
            let roots = all_roots(&self.places)?;
            self.follow_checkouts(&roots);
            let mut state = self.state.lock().expect("no panic holding it");
            let known: HashSet<PathBuf> = roots.iter().map(|r| r.path.clone()).collect();
            state.facts.retain(|path, _| known.contains(path));
            for root in &roots {
                dirty.sizes.insert(root.path.clone());
                if let Some(checkout) = &root.live
                    && !state.facts.contains_key(&root.path)
                {
                    dirty.git.insert(checkout.top.clone());
                }
            }
            state.roots = roots;
        }
        let roots = self
            .state
            .lock()
            .expect("no panic holding it")
            .roots
            .clone();
        // Git, claims and manifests.
        let tops: Vec<&Root> = roots
            .iter()
            .filter(|r| {
                r.live
                    .as_ref()
                    .is_some_and(|c| dirty.git.contains(&c.top) || dirty.tree.contains(&c.top))
            })
            .collect();
        // What the daemon read of a worktree whose files alone changed.
        let before: HashMap<PathBuf, (Option<String>, Option<SideGit>)> = {
            let state = self.state.lock().expect("no panic holding it");
            tops.iter()
                .filter(|r| r.live.as_ref().is_some_and(|c| !dirty.git.contains(&c.top)))
                .filter_map(|r| {
                    let facts = state.facts.get(&r.path)?;
                    Some((r.path.clone(), (facts.branch.clone(), facts.git.clone())))
                })
                .collect()
        };
        let read = parallel_map(&tops, |root| {
            let checkout = root.live.as_ref().expect("live");
            let manifest = build_dirs(&checkout.top).unwrap_or_default();
            let claim = if checkout.is_main() {
                Ok(None)
            } else {
                Claim::read(&checkout.claim_file())
            };
            let (branch, git) = match before.get(&root.path) {
                Some((branch, Some(SideGit::Side(Ok(read))))) => (
                    Ok(branch.clone()),
                    Some(SideGit::Side(crate::side_git::read_files(
                        &checkout.top,
                        read.clone(),
                    ))),
                ),
                _ => (
                    checkout.branch(),
                    Some(if checkout.is_main() {
                        SideGit::Main(crate::side_git::read_head(&checkout.top))
                    } else {
                        SideGit::Side(crate::side_git::read(&checkout.top))
                    }),
                ),
            };
            (root.path.clone(), manifest, claim, branch, git)
        });
        {
            let mut state = self.state.lock().expect("no panic holding it");
            for (path, manifest, claim, branch, git) in read {
                let facts = state.facts.entry(path.clone()).or_default();
                if facts.manifest != manifest {
                    dirty.sizes.insert(path.clone());
                }
                facts.manifest = manifest;
                facts.claim = claim?;
                facts.branch = branch?;
                facts.git = git;
            }
        }
        // The daemon reads each repo's branches again when any checkout of
        // the repo changed, since branches live in the repo's shared git
        // folder and the file events of every checkout include it. A repo
        // whose main checkout is gone has no branches to list.
        let live = crate::health::mains_of(&roots);
        {
            let mut state = self.state.lock().expect("no panic holding it");
            state.loose.retain(|main, _| live.contains(main));
        }
        let mains: Vec<PathBuf> = live
            .into_iter()
            .filter(|main| {
                roots
                    .iter()
                    .filter_map(|r| r.live.as_ref())
                    .any(|other| other.main == *main && dirty.git.contains(&other.top))
            })
            .collect();
        for main in mains {
            let mut judged = match self
                .state
                .lock()
                .expect("no panic holding it")
                .loose
                .remove(&main)
            {
                Some(BranchReading::Read(judged, _)) => judged,
                Some(BranchReading::Failed(_)) | None => Default::default(),
            };
            let reading = match crate::unlanded::loose_branches(&main, &mut judged) {
                Ok(loose) => BranchReading::Read(judged, loose),
                Err(error) => BranchReading::Failed(error.text()),
            };
            self.state
                .lock()
                .expect("no panic holding it")
                .loose
                .insert(main, reading);
        }
        // Sizes, from the records the commands that wrote the folders left
        // (sizes.rs); only a folder with no record is walked, once.
        for root in roots.iter().filter(|r| dirty.sizes.contains(&r.path)) {
            let manifest = self
                .state
                .lock()
                .expect("no panic holding it")
                .facts
                .get(&root.path)
                .map(|f| f.manifest.clone());
            let (folders, gone_kib) = match (&root.live, manifest) {
                (Some(checkout), Some(manifest)) => {
                    let lane = root.app_lane(&self.places)?;
                    let keys: Vec<String> = manifest.iter().map(|e| e.path.clone()).collect();
                    let answers = crate::sizes::known(&self.places, root, &keys)?;
                    let folders = manifest
                        .iter()
                        .zip(answers)
                        .map(|(entry, answer)| {
                            folder_facts(
                                entry,
                                entry.is_app_in(lane),
                                &root.path.join(&entry.path),
                                &checkout.top.join(&entry.path),
                                answer,
                            )
                        })
                        .collect();
                    (folders, None)
                }
                (Some(_), None) => continue,
                (None, _) => {
                    let answer =
                        crate::sizes::known(&self.places, root, &[String::new()])?.remove(0);
                    (Vec::new(), Some(answer.kib))
                }
            };
            let mut state = self.state.lock().expect("no panic holding it");
            let facts = state.facts.entry(root.path.clone()).or_default();
            facts.folders = Some(folders);
            facts.gone_kib = gone_kib;
        }
        self.publish();
        Ok(())
    }

    fn publish(&self) {
        let mut state = self.state.lock().expect("no panic holding it");
        state.version += 1;
        self.changed.notify_all();
    }

    /// A page started or stopped watching.
    pub fn watch(&self, watching: bool) {
        if watching {
            self.viewers.fetch_add(1, Ordering::SeqCst);
        } else {
            self.viewers.fetch_sub(1, Ordering::SeqCst);
        }
        let _guard = self.viewing_lock.lock().expect("no panic holding it");
        self.viewing.notify_all();
    }

    /// Reads the processes from the start, never inside an answer: a reading
    /// takes seconds on a loaded machine, longer than a command waits for the
    /// daemon. Pages watching make it faster.
    fn sample_loop(self: Arc<Self>) {
        loop {
            let reading = self.sampling.lock().expect("no panic holding it");
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            let read = self.sample_now();
            drop(reading);
            if let Err(error) = read {
                eprintln!(
                    "builds: reading the processes: {}",
                    match error {
                        crate::fail::Fail::Message(message) => message,
                        crate::fail::Fail::Exit(code) => format!("exit {code}"),
                    }
                );
            }
            // Pacing: the processes are read at a rate, not on an event. A
            // page that starts watching ends the wait.
            let pace = if self.viewers.load(Ordering::SeqCst) > 0 {
                SAMPLE_EVERY
            } else {
                IDLE_SAMPLE_EVERY
            };
            let guard = self.viewing_lock.lock().expect("no panic holding it");
            let _ = self.viewing.wait_timeout(guard, pace);
        }
    }

    /// Ends the process reading, after one in progress. A test calls it before
    /// it removes its scratch folder, which a reading writes into.
    #[cfg(test)]
    pub fn stop_sampling(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _guard = self.viewing_lock.lock().expect("no panic holding it");
        self.viewing.notify_all();
        drop(_guard);
        drop(self.sampling.lock().expect("no panic holding it"));
    }

    /// Asks the hosts for their load and busy processes on a thread of its
    /// own: an ssh round trip to a host that does not answer takes ten
    /// seconds, which no other reading waits for.
    fn hosts_loop(self: Arc<Self>) {
        let ssh = crate::host::Ssh::from_env();
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            match super::hosts::read(&self.places, &ssh) {
                Ok(mut hosts) => {
                    let now = now_seconds();
                    let mut state = self.state.lock().expect("no panic holding it");
                    state
                        .host_load
                        .retain(|name, _| hosts.iter().any(|host| host.name == *name));
                    for host in &mut hosts {
                        let kept = state.host_load.entry(host.name.clone()).or_default();
                        if let Some(load) = host.load {
                            kept.push_back((now, load[0]));
                        }
                        while kept
                            .front()
                            .is_some_and(|(at, _)| *at < now - HOST_LOAD_SECONDS)
                        {
                            kept.pop_front();
                        }
                        host.history = kept.iter().copied().collect();
                    }
                    if state.hosts != hosts {
                        state.hosts = hosts;
                        state.version += 1;
                        self.changed.notify_all();
                    }
                }
                Err(error) => crate::text::note(format!("reading the hosts: {}", error.text())),
            }
            // When a reading fails, a finding that says so replaces the last
            // reading's findings, so the page never shows old findings as
            // current.
            let found = match self.read_host_findings(&ssh) {
                Ok(found) => found,
                Err(error) => {
                    let text = format!(
                        "hosts: the daemon could not read the hosts' busy processes or which hosts refuse jobs: {}",
                        error.text()
                    );
                    crate::text::note(&text);
                    vec![crate::health::Finding::new(
                        crate::health::FindingKind::Host,
                        crate::health::Advice::said(text),
                    )]
                }
            };
            // The findings' reading added this round's samples.
            let (rates, rates_error) = match crate::sccache_history::read(&self.places.home) {
                Ok(samples) => (
                    crate::sccache_history::host_rates(&samples, now_seconds()),
                    None,
                ),
                Err(error) => {
                    let text = format!("reading the compile cache history: {}", error.text());
                    crate::text::note(&text);
                    (Vec::new(), Some(text))
                }
            };
            let mut state = self.state.lock().expect("no panic holding it");
            if state.host_findings != found {
                state.host_findings = found;
                state.version += 1;
                self.changed.notify_all();
            }
            if state.compile_cache != rates || state.compile_cache_error != rates_error {
                state.compile_cache = rates;
                state.compile_cache_error = rates_error;
                state.version += 1;
                self.changed.notify_all();
            }
            drop(state);
            let pace = if self.viewers.load(Ordering::SeqCst) > 0 {
                HOSTS_EVERY
            } else {
                IDLE_HOSTS_EVERY
            };
            let guard = self.viewing_lock.lock().expect("no panic holding it");
            let _ = self.viewing.wait_timeout(guard, pace);
        }
    }

    /// What `builds health` reads from the hosts besides the processes: each
    /// host's busy processes, this Mac's included, the hosts that refuse
    /// jobs, and what each host has installed.
    fn read_host_findings(
        &self,
        ssh: &crate::host::Ssh,
    ) -> crate::fail::Result<Vec<crate::health::Finding>> {
        let hosts = crate::host::all(&self.places)?;
        // This daemon is the dashboard.
        let dashboard = std::process::id() as i32;
        // Read before the scans, so a job that a process here started after
        // this reading counts as running (job_claim.rs).
        let holders = crate::job_claim::Holders::read()?;
        let mut scans = Vec::new();
        let mut busy = Vec::new();
        for (host, scan) in hosts
            .iter()
            .zip(crate::checkout::parallel_map(&hosts, |host| {
                crate::host_scan::read(ssh, host, &self.places.home)
            }))
        {
            busy.push(crate::stuck::read(
                ssh,
                host,
                &scan,
                host.target.is_none().then_some(dashboard),
            ));
            scans.push((host.clone(), scan));
        }
        let refusals = crate::host::refusals(&self.places)?;
        let installed = crate::host_installed::read_all_installed(&self.places, ssh, &hosts);
        Ok(crate::health::host_findings(
            &crate::health::Scanned::judge(&self.places, scans, &holders, now_seconds()),
            &busy,
            &refusals,
            &installed,
        ))
    }

    /// Judges health now from the last reading of the processes, with each
    /// claim read from its file now: a command that just wrote a claim
    /// (`note --state held`) asks for health next, before the next reading.
    /// None before the first reading. This function never reads the
    /// processes: a reading takes seconds on a loaded machine, longer than a
    /// command waits.
    pub fn health_now(&self) -> crate::fail::Result<Option<Health>> {
        let state = self.state.lock().expect("no panic holding it");
        match state.sample.clone() {
            Some(sample) => self.judge(&state, now_seconds(), sample).map(Some),
            None => Ok(None),
        }
    }

    /// Judges health at <now> from <sample>, the roots and their facts in
    /// <state>, and the claims as their files say now.
    fn judge(&self, state: &State, now: i64, sample: Sample) -> crate::fail::Result<Health> {
        let Sample {
            machine,
            places,
            disk,
        } = sample;
        let (_, loaded) = self.lists.current();
        let (mut claimed, held) = claimed_of(
            &self.places,
            &state.roots,
            &state.facts,
            &places,
            &machine.commits,
            &loaded,
        )?;
        let unclaimed = unclaimed_of(
            &self.places,
            &state.roots,
            &state.facts,
            &places,
            &crate::lists::kept_work::kept_worktrees(&loaded),
        )?;
        let lists = crate::health::lists_findings(
            &self.places,
            self.lists.integrity(),
            &loaded,
            &places,
            &mut claimed,
            &held,
            crate::health::WorktreeReads {
                designs: &self.design_reads,
                reviews: &self.review_reads,
            },
        )?;
        let mut loose: Vec<crate::unlanded::LooseBranch> = Vec::new();
        let mut unread: Vec<crate::health::Finding> = Vec::new();
        for (main, reading) in &state.loose {
            match reading {
                BranchReading::Read(_, branches) => loose.extend(branches.iter().cloned()),
                BranchReading::Failed(error) => {
                    unread.push(crate::health::branches_unread(main, error));
                }
            }
        }
        let mut health = assess(
            now,
            disk,
            machine,
            &places,
            claimed,
            crate::health::LeftBehind {
                worktrees: unclaimed,
                branches: crate::unlanded::unheld(loose, &loaded),
            },
            lists,
        );
        health
            .findings
            .extend(crate::main_checkout::findings(&self.places.home, &loaded));
        health.findings.extend(unread);
        Ok(health)
    }

    /// Reads the processes and judges health now.
    pub fn sample_now(&self) -> crate::fail::Result<Health> {
        let now = now_seconds();
        let (roots, sccache_due) = {
            let state = self.state.lock().expect("no panic holding it");
            (
                state.roots.clone(),
                state
                    .sccache_at
                    .is_none_or(|at| at.elapsed() >= SCCACHE_EVERY),
            )
        };
        let sccache = if sccache_due {
            let reading = sccache_reading(&self.places.home, now)?;
            let mut state = self.state.lock().expect("no panic holding it");
            state.sccache_at = Some(Instant::now());
            state.sccache = Some(reading.clone());
            reading
        } else {
            self.state
                .lock()
                .expect("no panic holding it")
                .sccache
                .clone()
                .expect("read before")
        };
        let places = places_of(&roots);
        let machine = sample(
            &places,
            &crate::host::all(&self.places)?,
            &locks_of(&self.places, &roots)?,
            &self.places,
            now,
            sccache,
        )?;
        let read_at = Moment::now();
        let free_kib = volume_space(&self.places.root)?.free_kib;
        let mut state = self.state.lock().expect("no panic holding it");
        let mut percent = HashMap::new();
        let mut cpu = HashMap::new();
        for process in &machine.builds {
            if let Some((before, at)) = state.cpu.get(&process.pid) {
                let seconds = read_at.duration_since(*at).as_secs_f64();
                if seconds > 0.0 {
                    percent.insert(
                        process.pid,
                        ((process.cpu_seconds - before) / seconds * 100.0).max(0.0),
                    );
                }
            }
            cpu.insert(process.pid, (process.cpu_seconds, read_at));
        }
        state.cpu = cpu;
        state.cpu_percent = percent;
        state.load.push_back((now, machine.load[0]));
        while state.load.len() > LOAD_KEPT {
            state.load.pop_front();
        }
        let sample = Sample {
            machine,
            places,
            disk: self.rules.disk_space(free_kib),
        };
        let health = self.judge(&state, now, sample.clone())?;
        state.sample = Some(sample);
        state.health = Some(health.clone());
        state.version += 1;
        self.changed.notify_all();
        Ok(health)
    }

    /// Waits until the state moved past <seen>, and returns the new version.
    pub fn wait_change(&self, seen: u64) -> u64 {
        let mut state = self.state.lock().expect("no panic holding it");
        while state.version <= seen {
            state = self.changed.wait(state).expect("no panic holding it");
        }
        state.version
    }

    /// The lists JSON the page reads: the stores as last read, and
    /// each running item's worktree joined with its claim and git. The same
    /// `Arc` comes back until one of those changes, so a caller sends the
    /// lists only when it is not the one it sent last.
    pub fn lists_json(&self) -> crate::fail::Result<Arc<String>> {
        let now = now_seconds();
        let (version, snapshot) = self.lists.current();
        let claims: Vec<(String, ClaimFacts)> = running_worktrees(&snapshot)
            .into_iter()
            .map(|worktree| (worktree.clone(), self.claim_facts(&worktree)))
            .collect();
        let reviews = self.design_reviews(&snapshot);
        let key: ServedKey = (
            version,
            crate::text::local_day(now),
            claims.iter().map(|(_, facts)| facts.clone()).collect(),
            reviews,
        );
        let mut served = self.served.lock().expect("no panic holding it");
        if let Some((built, json)) = &*served
            && *built == key
        {
            return Ok(json.clone());
        }
        let reviews = &key.3;
        let view = compose(
            &snapshot,
            now,
            &|worktree| {
                claims
                    .iter()
                    .find(|(w, _)| w == worktree)
                    .map(|(_, facts)| facts.clone())
                    .expect("every running worktree was asked")
            },
            &|project, id| {
                reviews
                    .iter()
                    .find(|(p, i, _)| p == project && *i == id)
                    .map(|(_, _, review)| review.clone())
                    .expect("every design change that waits on approval was asked")
            },
        );
        let json = Arc::new(
            serde_json::to_string(&super::lists::Served { projects: &view })
                .expect("the lists serialize"),
        );
        *served = Some((key, json.clone()));
        Ok(json)
    }

    /// The version of the stores as last read: it moves each time a read finds
    /// a store changed.
    pub fn lists_version(&self) -> u64 {
        self.lists.current().0
    }

    /// The hook runs' JSON the page reads; the same `Arc` comes back until
    /// the runs change.
    pub fn hooks_json(&self) -> Arc<String> {
        self.lists.hooks()
    }

    /// The review page of open design change <id>, drawn now, with the
    /// scratch boards in the change's worktree when <here> (`scratch_json`);
    /// None when no open design change has that id.
    pub fn design_page(
        &self,
        project: &str,
        id: u32,
        here: bool,
    ) -> Option<crate::fail::Result<String>> {
        let (_, snapshot) = self.lists.current();
        let (design, view) = super::lists::open_design(&snapshot, project, id)?;
        let scratch = scratch_json(here, || {
            let Some(main) = self.design_checkout(project) else {
                crate::fail!(
                    "project {project} has no design files, so no worktree of its holds a scratch board"
                );
            };
            super::scratch_view::scratch_list_of(project, &main?, Path::new(&design.worktree))
        });
        Some(
            view.and_then(|view| {
                crate::design_render::review_page(&design, &view, project, &scratch)
            }),
        )
    }

    /// <side> of the <n>th entry of open design change <id>'s review page,
    /// drawn now: a board, or a component's preview; None when there is no
    /// such change or entry.
    pub fn design_board(
        &self,
        project: &str,
        id: u32,
        n: usize,
        side: crate::design_render::Side,
    ) -> Option<crate::fail::Result<String>> {
        let (_, snapshot) = self.lists.current();
        let (design, _) = super::lists::open_design(&snapshot, project, id)?;
        let entry = crate::design_render::review_board(&design, n)?;
        let cache = self.places.design_system_cache();
        Some(
            crate::design_render::source_of(&design, side).and_then(|from| match &entry.shown {
                crate::lists::Shown::Board { path, theme } => {
                    crate::design_render::render_board(from, path, *theme, &cache)
                }
                crate::lists::Shown::Component(name) => {
                    crate::design_render::preview_frame(from, name, &cache, true)
                }
            }),
        )
    }

    /// The proposal of the <k>th board place of open design change <id> that
    /// its review page does not show, drawn now; None when there is no such
    /// change or board.
    pub fn design_changed_board(
        &self,
        project: &str,
        id: u32,
        k: usize,
    ) -> Option<crate::fail::Result<String>> {
        let (_, snapshot) = self.lists.current();
        let (design, _) = super::lists::open_design(&snapshot, project, id)?;
        let board = crate::design_render::other_board(&design, k)?;
        Some(self.draw_listed_board(&design, board, crate::design_render::Side::Proposed))
    }

    /// The now of the <k>th board open design change <id> deletes, drawn now;
    /// None when there is no such change or board.
    pub fn design_deleted_board(
        &self,
        project: &str,
        id: u32,
        k: usize,
    ) -> Option<crate::fail::Result<String>> {
        let (_, snapshot) = self.lists.current();
        let (design, _) = super::lists::open_design(&snapshot, project, id)?;
        let board = crate::design_render::deleted_board(&design, k)?;
        Some(self.draw_listed_board(&design, board, crate::design_render::Side::Now))
    }

    /// <side> of <board>, a board that <design>'s review page lists by name
    /// rather than shows, drawn now in the dark, since the recorder gave it
    /// no theme.
    fn draw_listed_board(
        &self,
        design: &crate::lists::DesignChange,
        board: &str,
        side: crate::design_render::Side,
    ) -> crate::fail::Result<String> {
        crate::design_render::source_of(design, side).and_then(|from| {
            crate::design_render::render_board(
                from,
                board,
                crate::lists::Theme::Dark,
                &self.places.design_system_cache(),
            )
        })
    }

    /// The main checkout of the repo that holds project <project>'s design
    /// files (`lists::design_repo`), and the repo's globs; None when the
    /// lists hold no such project or the project has no design files.
    pub(super) fn design_repo(
        &self,
        project: &str,
    ) -> Option<crate::fail::Result<(PathBuf, Globs)>> {
        let (_, snapshot) = self.lists.current();
        let (repo, globs) = match super::lists::project_design_repo(&snapshot, project)? {
            Ok(found) => found?,
            Err(error) => return Some(Err(error)),
        };
        Some(Ok((
            self.places.home.join(crate::host::clone_path(&repo)),
            globs,
        )))
    }

    /// The main checkout of the repo that holds project <project>'s design
    /// files, as `design_repo` finds it.
    fn design_checkout(&self, project: &str) -> Option<crate::fail::Result<PathBuf>> {
        Some(self.design_repo(project)?.map(|(main, _)| main))
    }

    /// The Designs page of project <project>, drawn now from origin/main,
    /// with the scratch boards of the repo's worktrees when <here>
    /// (`scratch_json`); None when the lists hold no such project or it
    /// has no design files.
    pub fn designs_page(&self, project: &str, here: bool) -> Option<crate::fail::Result<String>> {
        Some(self.design_checkout(project)?.and_then(|repo| {
            super::designs::page(
                project,
                &repo,
                &self.places.design_system_cache(),
                super::designs::standing(&self.places, project),
                &scratch_json(here, || super::scratch_view::scratch_list(project, &repo)),
            )
        }))
    }

    /// Asks the design session listening for project <project> to publish its
    /// designs at <rev>; None when the lists hold no such project or it has
    /// no design files.
    pub fn ask_publish(
        &self,
        project: &str,
        rev: &str,
    ) -> Option<crate::fail::Result<crate::lists::publish::Request>> {
        Some(
            self.design_checkout(project)?.and_then(|repo| {
                super::designs::ask(&self.places, project, &repo, rev, now_seconds())
            }),
        )
    }

    /// Board <name> of project <project>'s design files at <commit>, as a
    /// frame of the Designs page loads it.
    pub fn designs_board(
        &self,
        project: &str,
        commit: &str,
        name: &str,
    ) -> Option<crate::fail::Result<String>> {
        Some(self.design_checkout(project)?.and_then(|repo| {
            super::designs::board(&repo, commit, name, &self.places.design_system_cache())
        }))
    }

    /// Group <n> of project <project>'s colors at <commit>, as a frame of
    /// the Designs page loads it.
    pub fn designs_colors(
        &self,
        project: &str,
        commit: &str,
        n: usize,
    ) -> Option<crate::fail::Result<String>> {
        Some(self.design_checkout(project)?.and_then(|repo| {
            super::designs::color_group(&repo, commit, n, &self.places.design_system_cache())
        }))
    }

    /// Project <project>'s colors' comparison at <commit>, as a frame of the
    /// Designs page loads it.
    pub fn designs_colors_comparison(
        &self,
        project: &str,
        commit: &str,
    ) -> Option<crate::fail::Result<String>> {
        Some(self.design_checkout(project)?.and_then(|repo| {
            super::designs::color_comparison(&repo, commit, &self.places.design_system_cache())
        }))
    }

    /// Component <name>'s preview of project <project>'s design system at
    /// <commit>, as a frame of the Designs page loads it.
    pub fn designs_preview(
        &self,
        project: &str,
        commit: &str,
        name: &str,
    ) -> Option<crate::fail::Result<String>> {
        Some(self.design_checkout(project)?.and_then(|repo| {
            super::designs::preview(&repo, commit, name, &self.places.design_system_cache())
        }))
    }

    /// The name of the repo that holds project <project>'s design files and
    /// its main checkout, as `design_repo` finds them.
    fn design_repo_named(&self, project: &str) -> Option<crate::fail::Result<(String, PathBuf)>> {
        Some(self.design_checkout(project)?.and_then(|main| {
            let name = main
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    crate::fail::Fail::Message(format!("{} names no repo", main.display()))
                })?
                .to_string();
            Ok((name, main))
        }))
    }

    /// The page of component <name>'s shots in project <project>, at the
    /// commit origin/main names; <here> when it is asked for on this Mac.
    pub fn component_shots_page(
        &self,
        project: &str,
        name: &str,
        here: bool,
    ) -> Option<crate::fail::Result<String>> {
        Some(self.design_repo_named(project)?.and_then(|(repo, main)| {
            super::component_shots::page(&self.places, project, &repo, &main, name, here)
        }))
    }

    /// What the component page shows of component <name> of project
    /// <project> at <commit> (`component_shots::shots`).
    pub fn component_shots(
        &self,
        project: &str,
        commit: &str,
        name: &str,
    ) -> Option<crate::fail::Result<super::component_shots::Shots>> {
        Some(self.design_repo_named(project)?.and_then(|(repo, _)| {
            super::component_shots::shots(&self.places, &repo, commit, name, &|lane, prefix| {
                self.runs.shots_page(&repo, lane, prefix)
            })
        }))
    }

    /// Takes component <name>'s shots of <commit> of project <project>'s
    /// repo where builds has none cached (`component_shots::start`).
    pub fn capture_component(
        &self,
        project: &str,
        commit: &str,
        name: &str,
    ) -> Option<crate::fail::Result<()>> {
        Some(self.design_repo_named(project)?.and_then(|(repo, _)| {
            super::component_shots::start(&self.places, &self.runs, &repo, commit, name)
        }))
    }

    /// The <side> picture of the pair <shot> on <lane> in the run builds
    /// keeps of <commit> of project <project>'s repo.
    pub fn shot_picture(
        &self,
        project: &str,
        commit: &str,
        lane: crate::lane::Lane,
        shot: &str,
        side: super::component_shots::Side,
    ) -> Option<crate::fail::Result<Option<Vec<u8>>>> {
        Some(self.design_repo_named(project)?.and_then(|(repo, _)| {
            super::component_shots::picture(&self.places, &repo, commit, lane, shot, side)
        }))
    }

    /// Where the rules review of each open design change that waits on the
    /// person's approval stands, by project and id (`design_review`).
    fn design_reviews(
        &self,
        snapshot: &std::result::Result<Vec<crate::lists::store::Loaded>, String>,
    ) -> Vec<(String, crate::lists::Id, RulesReview)> {
        let Ok(stores) = snapshot else {
            return Vec::new();
        };
        let state = self.state.lock().expect("no panic holding it");
        let mut found = Vec::new();
        for loaded in stores {
            let Ok(lists) = &loaded.lists else { continue };
            for design in lists.designs.iter().filter(|d| d.awaits_approval()) {
                let review = self.design_review(&state, lists, design);
                found.push((lists.project.clone(), design.id, review));
            }
        }
        found
    }

    /// Where the rules review of open design change <design> of <lists>, which
    /// waits on the person's approval, stands: read when the change covers a
    /// commit that its worktree holds past origin/main, as <state> last read
    /// the worktree, and the rules review of the worktree's commits takes them
    /// as `builds land` would (decision #362); in review when no live
    /// worktree holds the commits it covers or the review does not take them;
    /// unread, with why, when git or the review's records can't be read.
    fn design_review(
        &self,
        state: &State,
        lists: &crate::lists::Lists,
        design: &crate::lists::DesignChange,
    ) -> RulesReview {
        let top = Path::new(&design.worktree);
        let seen = state.roots.iter().find_map(|root| {
            let live = root.live.as_ref().filter(|live| live.top == top)?;
            Some((live, state.facts.get(&root.path)?))
        });
        let Some((live, facts)) = seen else {
            return RulesReview::InReview;
        };
        let git = match &facts.git {
            Some(SideGit::Side(Ok(git))) => git,
            Some(SideGit::Side(Err(error))) => return RulesReview::Unread(error.clone()),
            _ => return RulesReview::InReview,
        };
        let repo = live.repo();
        match super::design_cards::standing(&self.design_reads, lists, &repo, top, git) {
            Ok(Some(super::design_cards::Standing::Approval(covering)))
                if covering.id == design.id => {}
            Ok(_) => return RulesReview::InReview,
            Err(error) => return RulesReview::Unread(error.text()),
        }
        let worktree = crate::review::verdicts::Worktree {
            repo: &repo,
            top,
            branch: facts.branch.as_deref(),
            git,
        };
        match self.rules_review(lists, &worktree) {
            Ok(None) => RulesReview::Read,
            Ok(Some(_)) => RulesReview::InReview,
            Err(error) => RulesReview::Unread(error),
        }
    }

    /// Where the rules review of the commits of <worktree>, a worktree of the
    /// project of <lists>, stands (`review_cards::standing`): None when
    /// land would take them as far as the review goes, or when they need no
    /// review.
    fn rules_review(
        &self,
        lists: &crate::lists::Lists,
        worktree: &crate::review::verdicts::Worktree,
    ) -> std::result::Result<Option<ItemWait>, String> {
        crate::lists::store::Store::named(&self.places, &lists.project)
            .map_err(crate::fail::Fail::text)
            .and_then(|store| {
                super::review_cards::standing(
                    &self.review_reads,
                    &store,
                    lists.version,
                    &self.places.claude_sessions(),
                    worktree,
                )
            })
    }

    /// The claim and git facts of the worktree at <worktree>; no agent and no
    /// commits for one the daemon doesn't know as a live checkout.
    fn claim_facts(&self, worktree: &str) -> ClaimFacts {
        let state = self.state.lock().expect("no panic holding it");
        let facts = state
            .roots
            .iter()
            .find(|root| {
                root.live
                    .as_ref()
                    .is_some_and(|checkout| checkout.top == Path::new(worktree))
            })
            .and_then(|root| state.facts.get(&root.path));
        let claim = facts.and_then(|f| f.claim.as_ref());
        ClaimFacts {
            agent: claim.map(|c| c.who.clone()),
            note: claim.and_then(|c| c.note_text().map(str::to_string)),
            note_state: claim.and_then(|c| c.state().map(str::to_string)),
            waiting_to_land: facts
                .and_then(|f| f.git.as_ref())
                .and_then(SideGit::side)
                .map_or(0, |git| git.ahead.len()),
        }
    }

    /// The whole state, as the page reads it.
    pub fn page(&self) -> crate::fail::Result<Page> {
        let space = volume_space(&self.places.root)?;
        let (_, snapshot) = self.lists.current();
        let state = self.state.lock().expect("no panic holding it");
        let machine = state.sample.as_ref().map(|sample| &sample.machine);
        let mut repos: Vec<RepoPage> = Vec::new();
        for root in &state.roots {
            let repo_name = root
                .path
                .parent()
                .map(crate::checkout::file_name)
                .unwrap_or_default();
            if !repos.iter().any(|r| r.name == repo_name) {
                repos.push(RepoPage {
                    lanes: crate::status::lanes_of(&self.places, &repo_name)?,
                    locks: machine
                        .map(|m| repo_locks(m, &repo_name))
                        .unwrap_or_default(),
                    queues: machine
                        .map(|m| repo_queues(m, &repo_name))
                        .unwrap_or_default(),
                    runs: self.runs.pages(&repo_name, &|lane| {
                        machine
                            .and_then(|m| {
                                m.queues
                                    .iter()
                                    .find(|q| q.repo == repo_name && q.lane == lane)
                            })
                            .map(|q| q.queue.iter().map(|waiting| waiting.pid).collect())
                            .unwrap_or_default()
                    }),
                    name: repo_name.clone(),
                    app_builds: Vec::new(),
                    checkouts: Vec::new(),
                });
            }
            let facts = state.facts.get(&root.path).cloned().unwrap_or_default();
            // Each build's place is an index into the sample's list of
            // places, so the page looks this root up in that list.
            let place = state
                .sample
                .as_ref()
                .and_then(|sample| sample.places.iter().position(|p| p.root == root.path));
            let processes: Vec<ProcessPage> = match (machine, place) {
                (Some(machine), Some(at)) => machine
                    .builds
                    .iter()
                    .filter(|b| b.place == at)
                    .map(|b| ProcessPage {
                        pid: b.pid,
                        started: b.started,
                        name: b.name.clone(),
                        cpu: state.cpu_percent.get(&b.pid).copied(),
                        memory: b.footprint,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let mut checkout = match &root.live {
                Some(live) => {
                    let total = facts.folders.as_ref().map(|folders| {
                        folders
                            .iter()
                            .filter(|f| f.exists)
                            .map(|f| f.kib)
                            .sum::<u64>()
                    });
                    CheckoutPage {
                        name: root.name(),
                        path: root.top.display().to_string(),
                        root: root.path.display().to_string(),
                        live: true,
                        main: live.is_main(),
                        branch: facts.branch.clone(),
                        app_checkout: root.is_app_checkout(&self.places)?,
                        app_lane: root.app_lane(&self.places)?.map(crate::lane::Lane::word),
                        // The card's state, which `judge_card` gives below.
                        state: None,
                        claim: facts.claim.clone(),
                        git: facts.git.clone(),
                        folders: facts.folders.clone(),
                        total_kib: total,
                        activity: Activity {
                            cpu: processes.iter().filter_map(|p| p.cpu).sum(),
                            memory: processes.iter().map(|p| p.memory).sum(),
                            started: processes.iter().map(|p| p.started).min(),
                            processes,
                        },
                        words: Default::default(),
                        stalled: false,
                        landings: None,
                        errors: Vec::new(),
                    }
                }
                None => CheckoutPage {
                    name: root.name(),
                    path: root.top.display().to_string(),
                    root: root.path.display().to_string(),
                    live: false,
                    main: false,
                    branch: None,
                    app_checkout: false,
                    app_lane: None,
                    state: None,
                    claim: None,
                    git: None,
                    folders: None,
                    total_kib: facts.gone_kib,
                    activity: Activity::default(),
                    words: Default::default(),
                    stalled: false,
                    landings: None,
                    errors: Vec::new(),
                },
            };
            let repo = repos
                .iter_mut()
                .find(|r| r.name == repo_name)
                .expect("added above");
            match &root.live {
                Some(live) if live.is_main() => {
                    checkout.words = super::cards::main_words(&checkout);
                }
                Some(live) if !checkout.app_checkout => {
                    let seen = Seen {
                        live,
                        name: &checkout.name,
                        repo: &repo.name,
                        claim: checkout.claim.as_ref(),
                        git: checkout.git.as_ref(),
                        branch: checkout.branch.as_deref(),
                    };
                    let judged = self.judge_card(
                        &snapshot,
                        &seen,
                        &repo.locks,
                        &repo.queues,
                        state.health.as_ref(),
                    );
                    checkout.stalled = state.health.as_ref().is_some_and(|h| {
                        h.worktrees
                            .iter()
                            .any(|w| w.stalled && w.path == checkout.path)
                    });
                    checkout.landings = state
                        .health
                        .as_ref()
                        .and_then(|h| h.worktrees.iter().find(|w| w.path == checkout.path))
                        .map(|w| w.landings.clone());
                    checkout.errors.extend(judged.errors);
                    checkout.state = Some(judged.state.word());
                    checkout.words = match &judged.kept {
                        Some((item, card)) => {
                            super::cards::kept_words(&checkout, item.clone(), card)
                        }
                        None => super::cards::words(
                            &checkout,
                            judged.state,
                            &judged.items,
                            &judged.waits,
                        ),
                    };
                }
                // A gone root and an app checkout have no card.
                _ => {}
            }
            repo.checkouts.push(checkout);
        }
        for repo in &mut repos {
            repo.app_builds = super::app_builds::build(repo)?;
        }
        Ok(Page {
            version: state.version,
            generated_at: now_seconds(),
            root: self.places.root.display().to_string(),
            volume: VolumePage {
                disk: self.rules.disk_space(space.free_kib),
                size_kib: space.size_kib,
            },
            load: machine.map(|m| LoadPage {
                now: m.load,
                cores: m.cores,
                history: state.load.iter().copied().collect(),
            }),
            health: state.health.clone().map(|mut health| {
                health.findings.extend(state.host_findings.iter().cloned());
                health
            }),
            hosts: state.hosts.clone(),
            compile_cache: state.compile_cache.clone(),
            compile_cache_error: state.compile_cache_error.clone(),
            repos,
        })
    }
}

impl Model {
    /// The cards of the live side worktrees that are not app checkouts, as
    /// the dashboard judges them now (`judge_card`): each with the project whose
    /// lists it shows under; None before the daemon's first reading of the
    /// processes, which says where agent shells are active and which app
    /// builds run. The recorder of the cards' states (card_recorder.rs)
    /// reads them.
    pub fn cards(&self) -> crate::fail::Result<Option<Vec<CardNow>>> {
        let (_, snapshot) = self.lists.current();
        let state = self.state.lock().expect("no panic holding it");
        let Some(machine) = state.sample.as_ref().map(|sample| &sample.machine) else {
            return Ok(None);
        };
        let mut cards = Vec::new();
        for root in &state.roots {
            let Some(live) = &root.live else { continue };
            if live.is_main() || root.is_app_checkout(&self.places)? {
                continue;
            }
            let Some(facts) = state.facts.get(&root.path) else {
                // The daemon has not read the worktree yet.
                cards.push(CardNow {
                    worktree: live.top.display().to_string(),
                    lists: CardLists::Unread,
                    state: CardState::WaitsUnread,
                    items: Vec::new(),
                });
                continue;
            };
            let repo = live.repo();
            let locks = repo_locks(machine, &repo);
            let queues = repo_queues(machine, &repo);
            let name = root.name();
            let seen = Seen {
                live,
                name: &name,
                repo: &repo,
                claim: facts.claim.as_ref(),
                git: facts.git.as_ref(),
                branch: facts.branch.as_deref(),
            };
            let judged = self.judge_card(&snapshot, &seen, &locks, &queues, state.health.as_ref());
            cards.push(CardNow {
                worktree: live.top.display().to_string(),
                lists: judged.lists,
                state: judged.state,
                items: match &judged.kept {
                    Some((item, _)) => vec![item.id],
                    None => judged.items.iter().map(|item| item.id).collect(),
                },
            });
        }
        Ok(Some(cards))
    }

    /// Judges the card of the live side worktree <seen>, whose repo's app
    /// build lanes have <locks> and <queues>, from the lists in <snapshot>
    /// and the last health reading <health>. The dashboard draws its card
    /// from what this returns, and the recorder of the cards' states records
    /// its state, so the two never disagree.
    fn judge_card(
        &self,
        snapshot: &std::result::Result<Vec<crate::lists::store::Loaded>, String>,
        seen: &Seen,
        locks: &[crate::health::LockHealth],
        queues: &[crate::health::LaneQueue],
        health: Option<&Health>,
    ) -> Judged {
        let mut errors = Vec::new();
        let top = Path::new(&seen.live.top);
        let path = seen.live.top.display().to_string();
        let claimed = match seen.live.claimed_project() {
            Ok(claimed) => Ok(claimed),
            Err(error) => Err(error.text()),
        };
        let project = claimed.and_then(|claimed| {
            crate::lists::store::lists_of(snapshot, top, seen.repo, claimed.as_deref())
        });
        let (project, lists) = match project {
            Ok(Some(project)) => (Some(project), CardLists::Project(project.project.clone())),
            Ok(None) => (None, CardLists::None),
            Err(error) => {
                errors.push(format!(
                    "The daemon can't choose this worktree's lists: {error}"
                ));
                (None, CardLists::Unread)
            }
        };
        let mut worktree = worktree_state(seen.claim, seen.git);
        let items = project
            .map(|lists| super::lists::running_items(lists, &path))
            .unwrap_or_default();
        let design = match (project, seen.git) {
            (Some(lists), Some(SideGit::Side(Ok(git)))) => {
                match super::design_cards::standing(&self.design_reads, lists, seen.repo, top, git)
                {
                    Ok(standing) => standing,
                    Err(error) => {
                        errors.push(format!(
                            "The daemon can't read which design changes cover this worktree's commits: {}",
                            error.text()
                        ));
                        None
                    }
                }
            }
            _ => None,
        };
        let awaits_approval = matches!(design, Some(super::design_cards::Standing::Approval(_)));
        let ready = worktree == CardState::ReadyToLand;
        // The card shows the rules review only between commits waiting to land
        // and Ready to land. A design change that waits on the person's
        // approval waits on their review only once the review takes the
        // commits, so its worktree's review is read whatever the card's state.
        let rules = match (project, seen.git) {
            (Some(lists), Some(SideGit::Side(Ok(git)))) if ready || awaits_approval => {
                let reviewed = crate::review::verdicts::Worktree {
                    repo: seen.repo,
                    top,
                    branch: seen.branch,
                    git,
                };
                Some(self.rules_review(lists, &reviewed))
            }
            _ => None,
        };
        let review_takes = matches!(rules, Some(Ok(None)));
        let design = design.and_then(|design| super::design_cards::wait(&design, review_takes));
        let review = match rules {
            Some(Ok(standing)) if ready => standing,
            Some(Err(error)) => {
                errors.push(format!(
                    "The daemon can't read the rules review of this worktree's commits: {error}"
                ));
                if ready {
                    // The review the daemon could not read may not take the
                    // commits, so the card says Needs review, never Ready to land.
                    worktree = CardState::InReview(ReviewWait::NotRun);
                }
                None
            }
            _ => None,
        };
        let build = seen
            .branch
            .and_then(|branch| super::app_builds::build_for(locks, queues, branch));
        let shell = health.is_some_and(|h| {
            h.shells
                .active
                .contains(&format!("{}/{}", seen.repo, seen.name))
        });
        let waits = match project
            .map(|lists| super::lists::running_waits(lists, &path))
            .transpose()
        {
            Ok(running) => Some(super::cards::ranked(
                running
                    .unwrap_or_default()
                    .into_iter()
                    .chain(design)
                    .chain(build)
                    .chain(review),
            )),
            Err(error) => {
                errors.push(format!(
                    "The daemon can't read what this worktree's running items wait on: {error}"
                ));
                None
            }
        };
        // A worktree an item keeps, with no agent, holds that item's work:
        // its card says what the item waits on.
        let kept = project
            .and_then(|lists| {
                crate::lists::kept_work::kept_in(lists, &path).map(|item| (lists, item))
            })
            .filter(|_| seen.claim.is_none())
            .map(|(lists, item)| {
                let running = RunningItem {
                    id: item.id.0,
                    title: item.title.clone(),
                };
                match super::lists::kept_card(lists, item) {
                    Ok(card) => Ok((running, card)),
                    Err(error) => {
                        errors.push(format!(
                            "The daemon can't read what the item this worktree keeps waits on: {error}"
                        ));
                        Err(running)
                    }
                }
            });
        let state = super::cards::card_state(
            worktree,
            shell,
            waits.as_deref(),
            kept.as_ref()
                .map(|kept| kept.as_ref().map(|(_, card)| card).map_err(|_| ())),
        );
        let (items, kept) = match kept {
            Some(Ok(kept)) => (items, Some(kept)),
            // The card lists the item it could not read the waits of.
            Some(Err(running)) => (vec![running], None),
            None => (items, None),
        };
        Judged {
            lists,
            state,
            items,
            waits: waits.unwrap_or_default(),
            kept,
            errors,
        }
    }
}

/// What the daemon reads of a live side worktree to judge its card.
struct Seen<'a> {
    live: &'a Checkout,
    name: &'a str,
    repo: &'a str,
    claim: Option<&'a Claim>,
    git: Option<&'a SideGit>,
    branch: Option<&'a str>,
}

/// A side worktree's card as `Model::judge_card` finds it.
struct Judged {
    /// The project whose lists the card shows under.
    lists: CardLists,
    state: CardState,
    /// The items it lists: the running items, or the item that keeps it
    /// when the daemon could not read that item's waits.
    items: Vec<RunningItem>,
    /// What it waits on, as `cards::ranked` orders it.
    waits: Vec<ItemWait>,
    /// The item that keeps it with no agent, and what its waits stand at.
    kept: Option<(RunningItem, KeptCard)>,
    /// What the daemon could not read, which the card shows.
    errors: Vec<String>,
}

/// The lists a card shows under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardLists {
    /// Those of this project.
    Project(String),
    /// None: no project holds the worktree's repo.
    None,
    /// The daemon could not choose the project, and the card says why.
    Unread,
}

/// A side worktree's card now, for the recorder of the cards' states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardNow {
    pub worktree: String,
    pub lists: CardLists,
    pub state: CardState,
    /// The ids of the items the card lists.
    pub items: Vec<u32>,
}

/// The app checkouts' locks of repo <repo> in <machine>, as the page and the
/// recorder of the cards' states read them.
/// The scratch boards that <list> lists, as a page shows them: null unless
/// <here>, since they are files that no commit holds, which the dashboard
/// shows only on this Mac; else `boards`, or the `error` that stops the
/// listing.
fn scratch_json(here: bool, list: impl FnOnce() -> crate::fail::Result<Vec<Value>>) -> Value {
    if !here {
        return Value::Null;
    }
    match list() {
        Ok(boards) => json!({ "boards": boards }),
        Err(error) => json!({ "error": error.text() }),
    }
}

fn repo_locks(machine: &Machine, repo: &str) -> Vec<crate::health::LockHealth> {
    machine
        .locks
        .iter()
        .filter(|lock| lock.repo == repo)
        .cloned()
        .collect()
}

/// The app build lanes' queues of repo <repo> in <machine>, as the page and
/// the recorder of the cards' states read them.
fn repo_queues(machine: &Machine, repo: &str) -> Vec<crate::health::LaneQueue> {
    machine
        .queues
        .iter()
        .filter(|queue| queue.repo == repo)
        .cloned()
        .collect()
}

fn folder_facts(
    entry: &BuildDir,
    app: bool,
    folder: &Path,
    link: &Path,
    answer: Measure,
) -> FolderFacts {
    let exists = folder.is_dir();
    let link_state = if link.is_symlink() {
        if absolute_link_target(link).is_ok_and(|target| target == folder) {
            "linked"
        } else {
            "elsewhere"
        }
    } else {
        "not linked"
    };
    FolderFacts {
        path: entry.path.clone(),
        cap_kib: entry.cap_kib(app),
        exists,
        kib: if exists { answer.kib } else { 0 },
        last_write: answer.newest,
        link: link_state,
    }
}

/// The claimed side worktrees of <roots>, with <commits>, the `git
/// commit`s that ran when the daemon last read the processes.
fn claimed_of(
    tool_places: &Places,
    roots: &[Root],
    facts: &HashMap<PathBuf, Facts>,
    places: &[Place],
    commits: &crate::procs::GitCommits,
    loaded: &std::result::Result<Vec<crate::lists::store::Loaded>, String>,
) -> crate::fail::Result<(Vec<Claimed>, Vec<crate::lists::held::Worktree>)> {
    let mut claimed = Vec::new();
    let mut held = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        // A main checkout's facts hold a main checkout's read.
        let Some(Facts {
            git: Some(SideGit::Side(git)),
            branch,
            ..
        }) = facts.get(&root.path)
        else {
            continue;
        };
        // The claim is read now, not taken from the facts: a command that
        // just wrote one (`note --state held`) asks for health next, before
        // the change has come through FSEvents.
        let claim = Claim::read(&checkout.claim_file())?;
        let Some(claim) = &claim else {
            continue;
        };
        let Some(place) = places.iter().position(|p| p.root == root.path) else {
            continue;
        };
        held.push(crate::lists::held::Worktree::read(
            tool_places,
            places[place].label.clone(),
            checkout,
            claim,
            branch.clone(),
            git,
            &crate::lists::held::Readings { commits, loaded },
        )?);
        claimed.push(Claimed {
            place,
            claim: claim.clone(),
            branch: branch.clone(),
            git: git.clone(),
            awaiting_review: false,
            landing: None,
        });
    }
    Ok((claimed, held))
}

/// The unclaimed side worktrees of <roots>, leaving out the app checkouts
/// and the worktrees in <kept>, which open items keep. When <kept>
/// is why builds cannot read the stores, each worktree carries that reason.
fn unclaimed_of(
    tool_places: &Places,
    roots: &[Root],
    facts: &HashMap<PathBuf, Facts>,
    places: &[Place],
    kept: &std::result::Result<Vec<String>, String>,
) -> crate::fail::Result<Vec<Unclaimed>> {
    let mut unclaimed = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if root.is_app_checkout(tool_places)?
            || kept
                .as_ref()
                .is_ok_and(|kept| kept.contains(&checkout.top.display().to_string()))
        {
            continue;
        }
        // A main checkout's facts hold a main checkout's read.
        let Some(SideGit::Side(git)) = facts.get(&root.path).and_then(|facts| facts.git.as_ref())
        else {
            continue;
        };
        if Claim::read(&checkout.claim_file())?.is_some() {
            continue;
        }
        let Some(place) = places.iter().position(|p| p.root == root.path) else {
            continue;
        };
        unclaimed.push(Unclaimed {
            place,
            git: git.clone(),
            kept_unknown: kept.as_ref().err().cloned(),
        });
    }
    Ok(unclaimed)
}

/// What the claim and commits of a live side worktree say of its card,
/// first match wins:
///
///     WaitingOnYou  its note says so: a question or step only the person
///                   can take
///     Paused        its note says held: kept on purpose with no work
///                   running, commits waiting to land or not
///     ReadyToLand   commits waiting to land, claimed or not: unclaimed
///                   ones are work an agent left behind
///     WindingDown   its note says so: the agent's last item, then its
///                   handoff
///     Working       claimed, none of the above
///     Free          unclaimed, nothing waiting to land
///
/// `cards::card_state` then gives the card its state from this and what the
/// worktree waits on.
pub fn worktree_state(claim: Option<&Claim>, git: Option<&SideGit>) -> CardState {
    let ahead = git
        .and_then(SideGit::side)
        .is_some_and(|g| !g.ahead.is_empty());
    let Some(claim) = claim else {
        return if ahead {
            CardState::ReadyToLand
        } else {
            CardState::Free
        };
    };
    if claim.state() == Some("waiting-on-you") {
        return CardState::WaitingOnYou;
    }
    if claim.is_held() {
        return CardState::Paused;
    }
    if ahead {
        return CardState::ReadyToLand;
    }
    if claim.state() == Some("winding-down") {
        return CardState::WindingDown;
    }
    CardState::Working
}

#[derive(Serialize)]
pub struct Page {
    pub version: u64,
    pub generated_at: i64,
    pub root: String,
    pub volume: VolumePage,
    pub load: Option<LoadPage>,
    /// The last health reading, with the hosts' findings after its own: the
    /// list `builds health` prints under "needs action".
    pub health: Option<Health>,
    /// Every recorded host besides this Mac, with its load.
    pub hosts: Vec<super::hosts::HostPage>,
    /// Each host's hit rate of Rust compiles over the last hour, day and
    /// week, this Mac's included.
    pub compile_cache: Vec<crate::sccache_history::HostRates>,
    /// Why the history behind `compile_cache` could not be read.
    pub compile_cache_error: Option<String>,
    pub repos: Vec<RepoPage>,
}

#[derive(Serialize)]
pub struct VolumePage {
    #[serde(flatten)]
    pub disk: DiskSpace,
    pub size_kib: u64,
}

#[derive(Serialize)]
pub struct LoadPage {
    pub now: [f64; 3],
    pub cores: usize,
    /// (seconds since 1970, 1-minute load) for the last ten minutes.
    pub history: Vec<(i64, f64)>,
}

#[derive(Serialize)]
pub struct RepoPage {
    pub name: String,
    pub lanes: Vec<crate::status::LaneJson>,
    /// Each app checkout's lock: its holder.
    pub locks: Vec<crate::health::LockHealth>,
    /// Each lane's one queue, for whichever of its locks frees first.
    pub queues: Vec<crate::health::LaneQueue>,
    /// The latest build-app run the page started in each lane.
    pub runs: Vec<RunPage>,
    /// The app checkouts as build lanes (app_builds.rs).
    pub app_builds: Vec<super::app_builds::AppLane>,
    pub checkouts: Vec<CheckoutPage>,
}

#[derive(Serialize)]
pub struct CheckoutPage {
    pub name: String,
    pub path: String,
    pub root: String,
    pub live: bool,
    pub main: bool,
    pub branch: Option<String>,
    pub app_checkout: bool,
    pub app_lane: Option<&'static str>,
    pub claim: Option<Claim>,
    pub git: Option<SideGit>,
    pub state: Option<&'static str>,
    /// None for a gone root, and for a live one until the daemon reads its
    /// sizes.
    pub folders: Option<Vec<FolderFacts>>,
    /// None until the daemon reads the sizes, and while the daemon is still
    /// walking a folder.
    pub total_kib: Option<u64>,
    pub activity: Activity,
    /// The words of the worktree's card (cards.rs); empty for a gone root
    /// and an app checkout, which have no card.
    pub words: super::cards::Words,
    /// Has health judged this claimed worktree stalled: no change and no
    /// build for an hour, and not held or waiting on the person's review
    /// (health.rs)? The card shows it.
    pub stalled: bool,
    /// The `builds land` processes that land in it, running or waiting for
    /// another landing (health.rs); the card shows them. None until health
    /// has read the worktree.
    pub landings: Option<Vec<crate::health::LandingRun>>,
    /// What the daemon could not read to say where the worktree's work
    /// stands, which the card shows: the claim's project, the project's
    /// lists, or its rules review.
    pub errors: Vec<String>,
}

#[derive(Default, Serialize)]
pub struct Activity {
    pub processes: Vec<ProcessPage>,
    /// CPU in percent of one core, summed.
    pub cpu: f64,
    pub memory: u64,
    /// When the oldest of the processes started, seconds since 1970.
    pub started: Option<i64>,
}

#[derive(Serialize)]
pub struct ProcessPage {
    pub pid: i32,
    pub started: i64,
    pub name: String,
    /// Percent of one core since the last sample; None on a process's first.
    pub cpu: Option<f64>,
    pub memory: u64,
}
