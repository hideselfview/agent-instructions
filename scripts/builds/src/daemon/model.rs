//! The daemon's live picture of every checkout, kept current from events:
//! the volume's (sizes, roots, app checkouts), the checkouts' own (git state,
//! claims, manifests), and, while a page watches, the processes'. Each part
//! is read again only when something it depends on changed, and every change
//! moves `version`, which the page's streams wait on.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::fsevents::Stream;
use super::lists::{Feed, compose, running_worktrees};
use super::runs::{RunPage, Runs};
use super::tracker::Tracker;
use crate::checkout::{Root, absolute_link_target, all_roots, parallel_map};
use crate::claim::Claim;
use crate::health::{Claimed, Health, Machine, Unclaimed, assess, locks_of, places_of, sample, sccache_reading};
use crate::manifest::{BuildDir, build_dirs};
use crate::places::{DiskSpace, Places, Rules};
use crate::procs::Place;
use crate::side_git::SideGit;
use crate::tool::volume_space;
use crate::lists::view::ClaimFacts;
use crate::usage::Measure;

/// Changes wait this long for more before they are read: agents write
/// files in bursts.
pub(super) const SETTLE: Duration = Duration::from_millis(300);
/// How often the processes are read while a page watches.
const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// How often sccache is asked for its count while a page watches.
const SCCACHE_EVERY: Duration = Duration::from_secs(30);
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
    /// Per manifest entry, in order.
    folders: Vec<FolderFacts>,
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

#[derive(Default)]
struct State {
    version: u64,
    roots: Vec<Root>,
    facts: HashMap<PathBuf, Facts>,
    machine: Option<Machine>,
    /// pid -> (CPU seconds, when read), for CPU %.
    cpu: HashMap<i32, (f64, Instant)>,
    cpu_percent: HashMap<i32, f64>,
    load: VecDeque<(i64, f64)>,
    health: Option<Health>,
    sccache_at: Option<Instant>,
    sccache: Option<(bool, Option<u64>, Option<crate::health::Reading>)>,
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
    git_stream: Mutex<Option<(Vec<PathBuf>, Stream)>>,
    lists: Arc<Feed>,
    /// The build-app runs the page starts.
    pub runs: Arc<Runs>,
    /// The lists JSON last built and what it was built from.
    served: Mutex<Option<(ServedKey, Arc<String>)>>,
}

/// What the lists JSON is built from besides the clock's second: the read
/// of the stores, the day, the mode and each running item's claim facts.
type ServedKey = (u64, String, &'static str, Vec<ClaimFacts>);

pub(super) fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs() as i64
}

impl Model {
    pub fn start(places: Places, rules: Rules, tracker: Arc<Tracker>, runs: Arc<Runs>) -> Arc<Model> {
        let model = Arc::new(Model {
            places: places.clone(),
            rules,
            tracker: tracker.clone(),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            dirty: Mutex::new(Dirty { layout: true, ..Dirty::default() }),
            wake: Condvar::new(),
            viewers: AtomicUsize::new(0),
            viewing: Condvar::new(),
            viewing_lock: Mutex::new(()),
            git_stream: Mutex::new(None),
            lists: Feed::start(places),
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
                let Ok(rest) = path.strip_prefix(volume) else { continue };
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
                    dirty.sizes.insert(self.places.root.join(&parts[0]).join(&parts[1]));
                }
            }
        });
    }

    /// The checkouts' own folders changed under <paths>.
    fn checkouts_changed(&self, paths: &[PathBuf]) {
        let roots = self.state.lock().expect("no panic holding it").roots.clone();
        self.mark(|dirty| {
            for path in paths {
                for root in &roots {
                    let Some(checkout) = &root.live else { continue };
                    let in_tree = path.starts_with(&checkout.top) && !path.starts_with(checkout.main.join(".git"));
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
                    if in_tree && path.parent().is_some_and(|p| p == checkout.top) || *path == checkout.top {
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
        if stream.as_ref().is_some_and(|(watched, _)| *watched == folders) {
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
                    let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                    model.checkouts_changed(&paths);
                }
            }),
        );
        match started {
            Some(started) => *stream = Some((folders, started)),
            None => eprintln!("builds: FSEvents would not follow the checkouts"),
        }
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
                eprintln!("builds: reading the checkouts: {}", match error {
                    crate::fail::Fail::Message(message) => message,
                    crate::fail::Fail::Exit(code) => format!("exit {code}"),
                });
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
        let roots = self.state.lock().expect("no panic holding it").roots.clone();
        // Git, claims and manifests.
        let tops: Vec<&Root> =
            roots.iter().filter(|r| r.live.as_ref().is_some_and(|c| dirty.git.contains(&c.top))).collect();
        let read = parallel_map(&tops, |root| {
            let checkout = root.live.as_ref().expect("live");
            let manifest = build_dirs(&checkout.top).unwrap_or_default();
            let claim = if checkout.is_main() { Ok(None) } else { Claim::read(&checkout.claim_file()) };
            let branch = checkout.branch();
            let git = if checkout.is_main() { None } else { Some(crate::side_git::read(&checkout.top)) };
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
        // Sizes, from the tracker's index: no disk is walked.
        for root in roots.iter().filter(|r| dirty.sizes.contains(&r.path)) {
            let manifest = self.state.lock().expect("no panic holding it").facts.get(&root.path).map(|f| f.manifest.clone());
            let (folders, gone_kib) = match (&root.live, manifest) {
                (Some(checkout), Some(manifest)) => {
                    let lane = root.app_lane(&self.places)?;
                    let paths: Vec<PathBuf> = manifest.iter().map(|e| root.path.join(&e.path)).collect();
                    let answers = self.tracker.measure(&paths, false);
                    let folders = manifest
                        .iter()
                        .zip(paths.iter().zip(answers))
                        .map(|(entry, (folder, answer))| {
                            folder_facts(entry, entry.is_app_in(lane), folder, &checkout.top.join(&entry.path), answer)
                        })
                        .collect();
                    (folders, None)
                }
                (Some(_), None) => continue,
                (None, _) => {
                    let answer = self.tracker.measure(std::slice::from_ref(&root.path), false).remove(0);
                    (Vec::new(), answer.map(|m| m.kib))
                }
            };
            let mut state = self.state.lock().expect("no panic holding it");
            let facts = state.facts.entry(root.path.clone()).or_default();
            facts.folders = folders;
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

    fn sample_loop(self: Arc<Self>) {
        loop {
            {
                let mut guard = self.viewing_lock.lock().expect("no panic holding it");
                while self.viewers.load(Ordering::SeqCst) == 0 {
                    guard = self.viewing.wait(guard).expect("no panic holding it");
                }
            }
            if let Err(error) = self.sample_now() {
                eprintln!("builds: reading the processes: {}", match error {
                    crate::fail::Fail::Message(message) => message,
                    crate::fail::Fail::Exit(code) => format!("exit {code}"),
                });
            }
            // Pacing: the processes are read at a rate, not on an event.
            std::thread::sleep(SAMPLE_EVERY);
        }
    }

    /// Reads the processes and judges health now.
    pub fn sample_now(&self) -> crate::fail::Result<Health> {
        let now = now_seconds();
        let (roots, sccache_due) = {
            let state = self.state.lock().expect("no panic holding it");
            (state.roots.clone(), state.sccache_at.is_none_or(|at| at.elapsed() >= SCCACHE_EVERY))
        };
        let sccache = if sccache_due {
            let reading = sccache_reading(&self.places.home, now)?;
            let mut state = self.state.lock().expect("no panic holding it");
            state.sccache_at = Some(Instant::now());
            state.sccache = Some(reading.clone());
            reading
        } else {
            self.state.lock().expect("no panic holding it").sccache.clone().expect("read before")
        };
        let places = places_of(&roots);
        let machine = sample(&places, &locks_of(&self.places, &roots)?, self.places.uid, &self.places.home, now, sccache);
        let read_at = Instant::now();
        let free_kib = volume_space(&self.places.root)?.free_kib;
        let mut state = self.state.lock().expect("no panic holding it");
        let mut percent = HashMap::new();
        let mut cpu = HashMap::new();
        for process in &machine.builds {
            if let Some((before, at)) = state.cpu.get(&process.pid) {
                let seconds = read_at.duration_since(*at).as_secs_f64();
                if seconds > 0.0 {
                    percent.insert(process.pid, ((process.cpu_seconds - before) / seconds * 100.0).max(0.0));
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
        let claimed = claimed_of(&roots, &state.facts, &places)?;
        let unclaimed = unclaimed_of(&self.places, &roots, &state.facts, &places)?;
        let health = assess(
            now,
            self.rules.disk_space(free_kib),
            machine.clone(),
            &places,
            claimed,
            unclaimed,
            crate::lists::running::findings(&self.lists.current().1, &roots)?,
        );
        state.machine = Some(machine);
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

    /// The lists JSON the page reads: the stores as last read, the mode, and
    /// each running item's worktree joined with its claim and git. The same
    /// `Arc` comes back until one of those changes, so a caller sends the
    /// lists only when it is not the one it sent last.
    pub fn lists_json(&self) -> crate::fail::Result<Arc<String>> {
        let now = now_seconds();
        let mode = crate::mode::current(&self.places)?;
        let (version, snapshot) = self.lists.current();
        let claims: Vec<(String, ClaimFacts)> =
            running_worktrees(&snapshot).into_iter().map(|worktree| (worktree.clone(), self.claim_facts(&worktree))).collect();
        let key: ServedKey = (version, crate::text::local_day(now), mode, claims.iter().map(|(_, facts)| facts.clone()).collect());
        let mut served = self.served.lock().expect("no panic holding it");
        if let Some((built, json)) = &*served
            && *built == key
        {
            return Ok(json.clone());
        }
        let view = compose(&snapshot, now, mode, &|worktree| {
            claims.iter().find(|(w, _)| w == worktree).map(|(_, facts)| facts.clone()).expect("every running worktree was asked")
        });
        let json = Arc::new(serde_json::to_string(&view).expect("the lists serialize"));
        *served = Some((key, json.clone()));
        Ok(json)
    }

    /// The render of the <n>th board of design change <id>: its content type
    /// and bytes. None when the change has no such render or the file is
    /// gone (the page then shows the board without it).
    pub fn design_render(&self, id: u32, n: usize) -> Option<(&'static str, Vec<u8>)> {
        let (_, snapshot) = self.lists.current();
        let (path, kind) = super::lists::design_render(&snapshot, id, n)?;
        std::fs::read(path).ok().map(|bytes| (kind, bytes))
    }

    /// The claim and git facts of the worktree at <worktree>; no agent and no
    /// commits for one the daemon doesn't know as a live checkout.
    fn claim_facts(&self, worktree: &str) -> ClaimFacts {
        let state = self.state.lock().expect("no panic holding it");
        let facts = state
            .roots
            .iter()
            .find(|root| root.live.as_ref().is_some_and(|checkout| checkout.top == Path::new(worktree)))
            .and_then(|root| state.facts.get(&root.path));
        let claim = facts.and_then(|f| f.claim.as_ref());
        ClaimFacts {
            agent: claim.map(|c| c.who.clone()),
            note: claim.and_then(|c| c.note.clone()),
            note_state: claim.and_then(|c| c.note_state.clone()),
            waiting_to_land: facts.and_then(|f| f.git.as_ref()).map_or(0, |git| git.ahead.len()),
        }
    }

    /// The whole state, as the page reads it.
    pub fn page(&self) -> crate::fail::Result<Page> {
        let space = volume_space(&self.places.root)?;
        let (entries, memory) = self.tracker.size();
        let state = self.state.lock().expect("no panic holding it");
        let places = places_of(&state.roots);
        let mut repos: Vec<RepoPage> = Vec::new();
        for root in &state.roots {
            let repo_name = root.path.parent().map(crate::checkout::file_name).unwrap_or_default();
            if !repos.iter().any(|r| r.name == repo_name) {
                repos.push(RepoPage {
                    lanes: crate::status::lanes_of(&self.places, &repo_name)?,
                    locks: state
                        .machine
                        .as_ref()
                        .map(|m| m.locks.iter().filter(|l| l.repo == repo_name).cloned().collect())
                        .unwrap_or_default(),
                    queues: state
                        .machine
                        .as_ref()
                        .map(|m| m.queues.iter().filter(|q| q.repo == repo_name).cloned().collect())
                        .unwrap_or_default(),
                    runs: self.runs.pages(&repo_name, &|lane| {
                        state
                            .machine
                            .as_ref()
                            .and_then(|m| m.queues.iter().find(|q| q.repo == repo_name && q.lane == lane.word()))
                            .map(|q| q.queue.iter().map(|waiting| waiting.pid).collect())
                            .unwrap_or_default()
                    }),
                    name: repo_name.clone(),
                    checkouts: Vec::new(),
                });
            }
            let facts = state.facts.get(&root.path).cloned().unwrap_or_default();
            let place = places.iter().position(|p| p.root == root.path);
            let processes: Vec<ProcessPage> = match (&state.machine, place) {
                (Some(machine), Some(at)) => machine
                    .builds
                    .iter()
                    .filter(|b| b.place == at)
                    .map(|b| ProcessPage {
                        pid: b.pid,
                        name: b.name.clone(),
                        cpu: state.cpu_percent.get(&b.pid).copied(),
                        memory: b.footprint,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let checkout = match &root.live {
                Some(live) => {
                    let total = facts.folders.iter().try_fold(0u64, |sum, f| if f.exists { f.kib.map(|k| sum + k) } else { Some(sum) });
                    CheckoutPage {
                        name: root.name(),
                        path: root.top.display().to_string(),
                        root: root.path.display().to_string(),
                        live: true,
                        main: live.is_main(),
                        branch: facts.branch.clone(),
                        app_checkout: root.is_app_checkout(&self.places)?,
                        app_lane: root.app_lane(&self.places)?.map(crate::lane::Lane::word),
                        state: (!live.is_main()).then(|| worktree_state(facts.claim.as_ref(), facts.git.as_ref())),
                        claim: facts.claim.clone(),
                        git: facts.git.clone(),
                        folders: Some(facts.folders.clone()),
                        total_kib: total,
                        activity: Activity {
                            cpu: processes.iter().filter_map(|p| p.cpu).sum(),
                            memory: processes.iter().map(|p| p.memory).sum(),
                            processes,
                        },
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
                },
            };
            repos.iter_mut().find(|r| r.name == repo_name).expect("added above").checkouts.push(checkout);
        }
        Ok(Page {
            version: state.version,
            generated_at: now_seconds(),
            root: self.places.root.display().to_string(),
            volume: VolumePage {
                disk: self.rules.disk_space(space.free_kib),
                size_kib: space.size_kib,
            },
            load: state.machine.as_ref().map(|m| LoadPage {
                now: m.load,
                cores: m.cores,
                history: state.load.iter().copied().collect(),
            }),
            daemon: DaemonPage { entries, memory_bytes: memory as u64 },
            health: state.health.clone(),
            repos,
        })
    }
}

fn folder_facts(entry: &BuildDir, app: bool, folder: &Path, link: &Path, answer: Option<Measure>) -> FolderFacts {
    let exists = folder.is_dir();
    let link_state = if link.is_symlink() {
        if absolute_link_target(link).is_ok_and(|target| target == folder) { "linked" } else { "elsewhere" }
    } else {
        "not linked"
    };
    FolderFacts {
        path: entry.path.clone(),
        cap_kib: entry.cap_kib(app),
        exists,
        kib: if exists { answer.map(|m| m.kib) } else { Some(0) },
        last_write: answer.and_then(|m| m.newest),
        link: link_state,
    }
}

fn claimed_of(roots: &[Root], facts: &HashMap<PathBuf, Facts>, places: &[Place]) -> crate::fail::Result<Vec<Claimed>> {
    let mut claimed = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if checkout.is_main() {
            continue;
        }
        let Some(facts) = facts.get(&root.path) else { continue };
        // The claim is read now, not taken from the facts: a command that
        // just wrote one (`note --state held`) asks for health next, before
        // the change has come through FSEvents.
        let claim = Claim::read(&checkout.claim_file())?;
        let (Some(claim), Some(git)) = (&claim, &facts.git) else { continue };
        let Some(place) = places.iter().position(|p| p.root == root.path) else { continue };
        claimed.push(Claimed { place, claim: claim.clone(), branch: facts.branch.clone(), git: git.clone() });
    }
    Ok(claimed)
}

fn unclaimed_of(
    tool_places: &Places,
    roots: &[Root],
    facts: &HashMap<PathBuf, Facts>,
    places: &[Place],
) -> crate::fail::Result<Vec<Unclaimed>> {
    let mut unclaimed = Vec::new();
    for root in roots {
        let Some(checkout) = &root.live else { continue };
        if checkout.is_main() || root.is_app_checkout(tool_places)? {
            continue;
        }
        let Some(git) = facts.get(&root.path).and_then(|facts| facts.git.as_ref()) else { continue };
        if Claim::read(&checkout.claim_file())?.is_some() {
            continue;
        }
        let Some(place) = places.iter().position(|p| p.root == root.path) else { continue };
        unclaimed.push(Unclaimed { place, git: git.clone() });
    }
    Ok(unclaimed)
}

/// The one state the page shows for a live side worktree, first match wins:
///
///     waiting-on-you  its note says so: a question or step only the person
///                     can take
///     landing         its claim's who ends in "(landing)": the worktree a
///                     coordinator lands commits from
///     held            its note says so: kept on purpose with no work
///                     running, commits waiting to land or not
///     ready-to-land   commits waiting to land, claimed or not: unclaimed
///                     ones are work an agent left behind
///     winding-down    its note says so: the agent's last item, then its
///                     handoff
///     working         claimed, none of the above
///     free            unclaimed, nothing waiting to land
pub fn worktree_state(claim: Option<&Claim>, git: Option<&SideGit>) -> &'static str {
    let ahead = git.is_some_and(|g| !g.ahead.is_empty());
    let Some(claim) = claim else {
        return if ahead { "ready-to-land" } else { "free" };
    };
    if claim.note_state.as_deref() == Some("waiting-on-you") {
        return "waiting-on-you";
    }
    if claim.who.ends_with("(landing)") {
        return "landing";
    }
    if claim.note_state.as_deref() == Some("held") {
        return "held";
    }
    if ahead {
        return "ready-to-land";
    }
    if claim.note_state.as_deref() == Some("winding-down") {
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
    pub health: Option<Health>,
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
    /// None for a gone root.
    pub folders: Option<Vec<FolderFacts>>,
    /// None while a folder is still being walked.
    pub total_kib: Option<u64>,
    pub activity: Activity,
}

#[derive(Default, Serialize)]
pub struct Activity {
    pub processes: Vec<ProcessPage>,
    /// CPU in percent of one core, summed.
    pub cpu: f64,
    pub memory: u64,
}

#[derive(Serialize)]
pub struct ProcessPage {
    pub pid: i32,
    pub name: String,
    /// Percent of one core since the last sample; None on a process's first.
    pub cpu: Option<f64>,
    pub memory: u64,
}
