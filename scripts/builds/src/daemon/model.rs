//! The daemon's live picture of every checkout, kept current from events:
//! the volume's (sizes, roots, app checkouts), the checkouts' own (git state,
//! claims, manifests), and, while a page watches, the processes'. Each part
//! is read again only when something it depends on changed, and every change
//! moves `version`, which the page's streams wait on.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::fsevents::{Stream, flags};
use super::lists::{Feed, compose, running_worktrees};
use super::runs::{RunPage, Runs};
use super::tracker::Tracker;
use crate::checkout::{Checkout, Root, absolute_link_target, all_roots, parallel_map};
use crate::claim::Claim;
use crate::health::{
    Claimed, Health, Machine, Unclaimed, assess, locks_of, places_of, sample, sccache_reading,
};
use crate::lists::view::ClaimFacts;
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
    /// Checkout tops whose git state, claim or manifest changed.
    git: HashSet<PathBuf>,
}

impl Dirty {
    fn any(&self) -> bool {
        self.layout || !self.sizes.is_empty() || !self.git.is_empty()
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
    /// None while the daemon is still walking it.
    pub kib: Option<u64>,
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
    cpu: HashMap<i32, (f64, Instant)>,
    cpu_percent: HashMap<i32, f64>,
    load: VecDeque<(i64, f64)>,
    health: Option<Health>,
    sccache_at: Option<Instant>,
    sccache: Option<(bool, Option<u64>, Option<crate::health::Reading>)>,
    /// The other hosts' load cards (hosts.rs), empty before the first reading.
    hosts: Vec<super::hosts::HostPage>,
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
    pub tracker: Arc<Tracker>,
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
    lists: Arc<Feed>,
    /// What git said the commits of design branches change.
    design_reads: super::design_cards::Reads,
    /// What git said of each branch's commits for its rules review.
    review_reads: super::review_cards::Reads,
    /// The build-app runs the page starts.
    pub runs: Arc<Runs>,
    /// The lists JSON last built and what it was built from.
    served: Mutex<Option<(ServedKey, Arc<String>)>>,
}

/// What the lists JSON is built from besides the clock's second: the read
/// of the stores, the day and each running item's claim facts.
type ServedKey = (u64, String, Vec<ClaimFacts>);

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
        tracker: Arc<Tracker>,
        runs: Arc<Runs>,
    ) -> Arc<Model> {
        let model = Arc::new(Model {
            places: places.clone(),
            rules,
            tracker: tracker.clone(),
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
            lists: Feed::start(places),
            design_reads: Default::default(),
            review_reads: Default::default(),
            runs,
            served: Mutex::new(None),
        });
        let weak = Arc::downgrade(&model);
        tracker.listen(Box::new(move |paths| {
            if let Some(model) = weak.upgrade() {
                model.volume_changed(paths);
            }
        }));
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

    /// The tracker handled writes under <paths>.
    fn volume_changed(&self, paths: &[PathBuf]) {
        let volume = &self.tracker.root;
        self.mark(|dirty| {
            for path in paths {
                let Ok(rest) = path.strip_prefix(volume) else {
                    continue;
                };
                let parts: Vec<String> = rest
                    .components()
                    .filter_map(|c| match c {
                        Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .collect();
                if parts.is_empty() {
                    // A whole walk: everything may have changed.
                    dirty.layout = true;
                    continue;
                }
                if parts.len() <= 2
                    || parts.get(2).is_some_and(|p| p == ".checkout")
                    || parts.get(1).is_some_and(|p| crate::lane::is_lane_file(p))
                {
                    dirty.layout = true;
                }
                if parts.len() >= 2 {
                    dirty
                        .sizes
                        .insert(self.places.root.join(&parts[0]).join(&parts[1]));
                }
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
                    if (in_tree && !checkout.is_main()) || in_own_git || in_common {
                        dirty.git.insert(checkout.top.clone());
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
            .filter(|r| r.live.as_ref().is_some_and(|c| dirty.git.contains(&c.top)))
            .collect();
        let read = parallel_map(&tops, |root| {
            let checkout = root.live.as_ref().expect("live");
            let manifest = build_dirs(&checkout.top).unwrap_or_default();
            let claim = if checkout.is_main() {
                Ok(None)
            } else {
                Claim::read(&checkout.claim_file())
            };
            let branch = checkout.branch();
            let git = Some(if checkout.is_main() {
                crate::side_git::read_head(&checkout.top)
            } else {
                crate::side_git::read(&checkout.top)
            });
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
        // Sizes, from the tracker's index: no disk is walked.
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
                    let paths: Vec<PathBuf> =
                        manifest.iter().map(|e| root.path.join(&e.path)).collect();
                    let answers = self.tracker.measure(&paths, false);
                    let folders = manifest
                        .iter()
                        .zip(paths.iter().zip(answers))
                        .map(|(entry, (folder, answer))| {
                            folder_facts(
                                entry,
                                entry.is_app_in(lane),
                                folder,
                                &checkout.top.join(&entry.path),
                                answer,
                            )
                        })
                        .collect();
                    (folders, None)
                }
                (Some(_), None) => continue,
                (None, _) => {
                    let answer = self
                        .tracker
                        .measure(std::slice::from_ref(&root.path), false)
                        .remove(0);
                    (Vec::new(), answer.map(|m| m.kib))
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
            let mut state = self.state.lock().expect("no panic holding it");
            if state.host_findings != found {
                state.host_findings = found;
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
        let busy = crate::checkout::parallel_map(&hosts, |host| {
            crate::stuck::read(ssh, host, host.target.is_none().then_some(dashboard))
        });
        let refusals = crate::host::refusals(&self.places)?;
        let installed = crate::host_installed::read_all_installed(&self.places, ssh, &hosts);
        Ok(crate::health::host_findings(&busy, &refusals, &installed))
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
        let (mut claimed, held) = claimed_of(
            &self.places,
            &state.roots,
            &state.facts,
            &places,
            &machine.commits,
        )?;
        let (_, loaded) = self.lists.current();
        let unclaimed = unclaimed_of(
            &self.places,
            &state.roots,
            &state.facts,
            &places,
            &crate::lists::kept_work::kept_worktrees(&loaded),
        )?;
        let lists =
            crate::health::lists_findings(&self.places, &loaded, &places, &mut claimed, &held)?;
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
        let read_at = Instant::now();
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
        let key: ServedKey = (
            version,
            crate::text::local_day(now),
            claims.iter().map(|(_, facts)| facts.clone()).collect(),
        );
        let mut served = self.served.lock().expect("no panic holding it");
        if let Some((built, json)) = &*served
            && *built == key
        {
            return Ok(json.clone());
        }
        let view = compose(&snapshot, now, &|worktree| {
            claims
                .iter()
                .find(|(w, _)| w == worktree)
                .map(|(_, facts)| facts.clone())
                .expect("every running worktree was asked")
        });
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

    /// The review page of open design change <id>, drawn now; None when no
    /// open design change has that id.
    pub fn design_page(&self, project: &str, id: u32) -> Option<crate::fail::Result<String>> {
        let (_, snapshot) = self.lists.current();
        let (design, view) = super::lists::open_design(&snapshot, project, id)?;
        Some(view.and_then(|view| crate::design_render::review_page(&design, &view, project)))
    }

    /// <side> of the <n>th board of open design change <id>'s review page,
    /// drawn now; None when there is no such change or board.
    pub fn design_board(
        &self,
        project: &str,
        id: u32,
        n: usize,
        side: crate::design_render::Side,
    ) -> Option<crate::fail::Result<String>> {
        let (_, snapshot) = self.lists.current();
        let (design, _) = super::lists::open_design(&snapshot, project, id)?;
        let board = crate::design_render::review_board(&design, n)?;
        Some(
            crate::design_render::source_of(&design, side).and_then(|from| {
                crate::design_render::render_board(
                    from,
                    &board.board,
                    board.theme,
                    &self.places.design_system_cache(),
                )
            }),
        )
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
        Some(
            crate::design_render::source_of(&design, crate::design_render::Side::Now).and_then(
                |from| {
                    crate::design_render::render_board(
                        from,
                        board,
                        crate::lists::Theme::Dark,
                        &self.places.design_system_cache(),
                    )
                },
            ),
        )
    }

    /// The main checkout of the repo that holds project <project>'s design
    /// files (`lists::design_repo`); None when the lists hold no such project
    /// or the project has no design files.
    fn design_checkout(&self, project: &str) -> Option<crate::fail::Result<PathBuf>> {
        let (_, snapshot) = self.lists.current();
        let repo = match super::lists::project_design_repo(&snapshot, project)? {
            Ok(repo) => repo?,
            Err(error) => return Some(Err(error)),
        };
        Some(Ok(self.places.home.join(crate::host::clone_path(&repo))))
    }

    /// The Designs page of project <project>, drawn now from origin/main;
    /// None when the lists hold no such project or it has no design files.
    pub fn designs_page(&self, project: &str) -> Option<crate::fail::Result<String>> {
        Some(self.design_checkout(project)?.and_then(|repo| {
            super::designs::page(
                project,
                &repo,
                &self.places.design_system_cache(),
                super::designs::standing(&self.places, project),
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
                .map_or(0, |git| git.ahead.len()),
        }
    }

    /// The whole state, as the page reads it.
    pub fn page(&self) -> crate::fail::Result<Page> {
        let space = volume_space(&self.places.root)?;
        let (entries, memory) = self.tracker.size();
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
                        .map(|m| {
                            m.locks
                                .iter()
                                .filter(|l| l.repo == repo_name)
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default(),
                    queues: machine
                        .map(|m| {
                            m.queues
                                .iter()
                                .filter(|q| q.repo == repo_name)
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default(),
                    runs: self.runs.pages(&repo_name, &|lane| {
                        machine
                            .and_then(|m| {
                                m.queues
                                    .iter()
                                    .find(|q| q.repo == repo_name && q.lane == lane.word())
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
            let checkout = match &root.live {
                Some(live) => {
                    let total = facts.folders.as_ref().and_then(|folders| {
                        folders.iter().try_fold(0u64, |sum, f| {
                            if f.exists {
                                f.kib.map(|k| sum + k)
                            } else {
                                Some(sum)
                            }
                        })
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
                        state: (!live.is_main())
                            .then(|| worktree_state(facts.claim.as_ref(), facts.git.as_ref())),
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
                    errors: Vec::new(),
                },
            };
            repos
                .iter_mut()
                .find(|r| r.name == repo_name)
                .expect("added above")
                .checkouts
                .push(checkout);
        }
        for repo in &mut repos {
            repo.app_builds = super::app_builds::build(repo)?;
            let builds: HashMap<String, super::cards::ItemWait> = repo
                .checkouts
                .iter()
                .filter(|c| c.live && !c.app_checkout && !c.main)
                .filter_map(|c| {
                    let wait = super::app_builds::build_for(repo, c.branch.as_deref()?)?;
                    Some((c.name.clone(), wait))
                })
                .collect();
            for checkout in &mut repo.checkouts {
                if checkout.live && !checkout.app_checkout {
                    let claimed = match state
                        .roots
                        .iter()
                        .filter_map(|root| root.live.as_ref())
                        .find(|live| live.top == Path::new(&checkout.path))
                        .map(Checkout::claimed_project)
                        .transpose()
                    {
                        Ok(claimed) => Ok(claimed.flatten()),
                        Err(error) => Err(error.text()),
                    };
                    let project = claimed.and_then(|claimed| {
                        crate::lists::store::lists_of(
                            &snapshot,
                            Path::new(&checkout.path),
                            &repo.name,
                            claimed.as_deref(),
                        )
                    });
                    let project = match project {
                        Ok(project) => project,
                        Err(error) => {
                            checkout.errors.push(format!(
                                "The daemon can't choose this worktree's lists: {error}"
                            ));
                            None
                        }
                    };
                    let items = project
                        .map(|lists| super::lists::running_items(lists, &checkout.path))
                        .unwrap_or_default();
                    let design = match (project, &checkout.git) {
                        (Some(lists), Some(git)) if !checkout.main && git.error.is_none() => {
                            let standing = self.design_reads.standing(
                                lists,
                                &repo.name,
                                Path::new(&checkout.path),
                                &git.ahead,
                            );
                            match standing {
                                Ok(standing) => standing,
                                Err(error) => {
                                    checkout.errors.push(format!(
                                        "The daemon can't read which design changes cover this worktree's commits: {}",
                                        error.text()
                                    ));
                                    None
                                }
                            }
                        }
                        _ => None,
                    };
                    // The rules review stands only between commits waiting
                    // to land and Ready to land.
                    let review = match (project, &checkout.git) {
                        (Some(lists), Some(git))
                            if checkout.state == Some("ready-to-land") && git.error.is_none() =>
                        {
                            let standing =
                                crate::lists::store::Store::named(&self.places, &lists.project)
                                    .map_err(crate::fail::Fail::text)
                                    .and_then(|store| {
                                        self.review_reads.standing(
                                            &store,
                                            Path::new(&checkout.path),
                                            &git.ahead,
                                        )
                                    });
                            match standing {
                                Ok(standing) => standing,
                                Err(error) => {
                                    checkout.errors.push(format!(
                                        "The daemon can't read the rules review of this worktree's commits: {error}"
                                    ));
                                    // The review the daemon could not read may not take the
                                    // commits, so the card says Needs review, never Ready to land.
                                    checkout.state = Some("in-review");
                                    None
                                }
                            }
                        }
                        _ => None,
                    };
                    let build = builds.get(&checkout.name).cloned();
                    checkout.stalled = state.health.as_ref().is_some_and(|h| {
                        h.worktrees
                            .iter()
                            .any(|w| w.stalled && w.path == checkout.path)
                    });
                    let shell = state.health.as_ref().is_some_and(|h| {
                        h.shells
                            .active
                            .contains(&format!("{}/{}", repo.name, checkout.name))
                    });
                    let waits = match project
                        .map(|lists| super::lists::running_waits(lists, &checkout.path))
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
                            checkout.errors.push(format!(
                                "The daemon can't read what this worktree's running items wait on: {error}"
                            ));
                            None
                        }
                    };
                    checkout.state =
                        super::cards::card_state(checkout.state, shell, waits.as_deref());
                    let waits = waits.unwrap_or_default();
                    // A worktree an item keeps, with no agent, holds that
                    // item's work: its card says what the item waits on.
                    let kept = project
                        .and_then(|lists| {
                            crate::lists::kept_work::kept_in(lists, &checkout.path)
                                .map(|item| (lists, item))
                        })
                        .filter(|_| checkout.claim.is_none());
                    checkout.words = match kept {
                        Some((lists, item)) => {
                            let running = super::cards::RunningItem {
                                id: item.id.0,
                                title: item.title.clone(),
                            };
                            let read = super::lists::kept_card(lists, item);
                            match read {
                                Ok(card) => {
                                    checkout.state = Some(super::cards::kept_state(&card));
                                    super::cards::kept_words(checkout, running, &card)
                                }
                                Err(error) => {
                                    checkout.errors.push(format!(
                                        "The daemon can't read what the item this worktree keeps waits on: {error}"
                                    ));
                                    checkout.state = Some("waits-unread");
                                    super::cards::words(checkout, &[running], &[])
                                }
                            }
                        }
                        None => super::cards::words(checkout, &items, &waits),
                    };
                }
            }
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
            daemon: DaemonPage {
                entries,
                memory_bytes: memory as u64,
            },
            health: state.health.clone().map(|mut health| {
                health.findings.extend(state.host_findings.iter().cloned());
                health
            }),
            hosts: state.hosts.clone(),
            repos,
        })
    }
}

fn folder_facts(
    entry: &BuildDir,
    app: bool,
    folder: &Path,
    link: &Path,
    answer: Option<Measure>,
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
        kib: if exists {
            answer.map(|m| m.kib)
        } else {
            Some(0)
        },
        last_write: answer.and_then(|m| m.newest),
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
) -> crate::fail::Result<(Vec<Claimed>, Vec<crate::lists::held::Worktree>)> {
    let mut claimed = Vec::new();
    let mut held = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if checkout.is_main() {
            continue;
        }
        let Some(facts) = facts.get(&root.path) else {
            continue;
        };
        // The claim is read now, not taken from the facts: a command that
        // just wrote one (`note --state held`) asks for health next, before
        // the change has come through FSEvents.
        let claim = Claim::read(&checkout.claim_file())?;
        let (Some(claim), Some(git)) = (&claim, &facts.git) else {
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
            git,
            commits,
        )?);
        claimed.push(Claimed {
            place,
            claim: claim.clone(),
            branch: facts.branch.clone(),
            git: git.clone(),
            awaiting_review: false,
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
        if checkout.is_main()
            || root.is_app_checkout(tool_places)?
            || kept
                .as_ref()
                .is_ok_and(|kept| kept.contains(&checkout.top.display().to_string()))
        {
            continue;
        }
        let Some(git) = facts.get(&root.path).and_then(|facts| facts.git.as_ref()) else {
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

/// The one state the page shows for a live side worktree, first match wins:
///
///     waiting-on-you  its note says so: a question or step only the person
///                     can take
///     held            its note says so: kept on purpose with no work
///                     running, commits waiting to land or not
///     ready-to-land   commits waiting to land, claimed or not: unclaimed
///                     ones are work an agent left behind. The page turns
///                     it into in-review while the rules review would keep
///                     `builds land` from taking them (cards::in_review)
///     winding-down    its note says so: the agent's last item, then its
///                     handoff
///     working         claimed, none of the above
///     free            unclaimed, nothing waiting to land
///
/// The page then turns an unclaimed worktree an open item keeps into
/// kept, whatever it holds (lists/kept_work.rs). The page also gives the state
/// waits-unread to a worktree when the daemon could not read what its running
/// items wait on (cards::card_state).
pub fn worktree_state(claim: Option<&Claim>, git: Option<&SideGit>) -> &'static str {
    let ahead = git.is_some_and(|g| !g.ahead.is_empty());
    let Some(claim) = claim else {
        return if ahead { "ready-to-land" } else { "free" };
    };
    if claim.state() == Some("waiting-on-you") {
        return "waiting-on-you";
    }
    if claim.is_held() {
        return "held";
    }
    if ahead {
        return "ready-to-land";
    }
    if claim.state() == Some("winding-down") {
        return "winding-down";
    }
    "working"
}

#[derive(Serialize)]
pub struct Page {
    pub version: u64,
    pub generated_at: i64,
    pub root: String,
    pub volume: VolumePage,
    pub load: Option<LoadPage>,
    pub daemon: DaemonPage,
    /// The last health reading, with the hosts' findings after its own: the
    /// list `builds health` prints under "needs action".
    pub health: Option<Health>,
    /// Every recorded host besides this Mac, with its load.
    pub hosts: Vec<super::hosts::HostPage>,
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
pub struct DaemonPage {
    pub entries: usize,
    pub memory_bytes: u64,
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
