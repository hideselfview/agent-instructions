//! The lists as the page reads them: the store's folder followed live, and
//! the lists JSON built from what was read and each
//! running item's worktree.
//!
//! `Feed` follows `Places::lists` with an FSEvents stream and, after the
//! settle delay, asks each store's database whether anything committed to it
//! since the last read (`db::Watch`). Events alone prove nothing: a review's
//! log and SQLite's own files sit in the same folders, and opening a
//! connection changes those files. When a database changed, the feed asks
//! each store for its last write's version: a store that has not moved is not
//! read again (a hook run's write reads no lists), and the reads that found a
//! store changed are counted. The hook runs' figures are read again whenever
//! a database changed, and each minute, since they count the runs of a window
//! that ends now. A store whose database changed gets SQLite's integrity
//! checks again (`Store::integrity`), which read the whole database. A store
//! that fails its check is served as the JSON's `error`; nothing repairs it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::fsevents::Stream;
use super::model::SETTLE;
use crate::lists::design_paths::Globs;
use crate::lists::store::{Loaded, database_paths, load_paths, reload, stores};
use crate::lists::view::{ClaimFacts, DesignView, RulesReview, View, design_view};
use crate::lists::{DesignChange, Id};
use crate::places::Places;

type Snapshot = Result<Vec<Loaded>, String>;

pub struct Feed {
    places: Places,
    /// The version counts reads that changed what is served; the snapshot is
    /// what the last of them read.
    current: Mutex<(u64, Arc<Snapshot>)>,
    /// The hook runs' JSON (hook_stats.rs) as the last read built it.
    hooks: Mutex<Arc<String>>,
    /// The minute (seconds since 1970 over 60) the hook runs' JSON was built
    /// in.
    hooks_minute: AtomicI64,
    /// The time the hook runs' 12-hour window ends at, in seconds since
    /// 1970; the wall clock, except in a test that sets it.
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    /// Each database file in a store's folder, by path, with what tells
    /// whether it changed.
    watches: Mutex<HashMap<PathBuf, crate::db::Watch>>,
    /// What the integrity checks found in each store's database when it last
    /// changed, by project.
    integrity: Mutex<BTreeMap<String, Vec<String>>>,
    dirty: AtomicBool,
    wake: Condvar,
    wake_lock: Mutex<()>,
    listener: Mutex<Option<Listener>>,
    stream: Mutex<Option<Stream>>,
}

impl Feed {
    /// Reads the stores and follows their folder. A folder that can't be
    /// made or followed is said on stderr, as the model does for the
    /// checkouts, and the stores are still served as read at start.
    pub fn start(places: Places) -> Arc<Feed> {
        Feed::start_with_clock(places, Box::new(crate::app_timing::now))
    }

    /// `start`, with <clock> telling the time the hook runs' window ends at.
    pub fn start_with_clock(
        places: Places,
        clock: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> Arc<Feed> {
        let dir: PathBuf = places.lists.clone();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("builds: creating {}: {error}", dir.display());
        }
        // Taken before the first read: a write after it is reported by the
        // stream, one before it is in the read.
        let since = Stream::current_event_id();
        // Opened before the first read, so a write after them moves them.
        let mut watches = HashMap::new();
        let snapshot =
            scan_databases(&places, &mut watches).and_then(|scan| load_paths(&places, &scan.paths));
        report_snapshot_fault(&snapshot, None);
        let now = clock();
        let feed = Arc::new(Feed {
            current: Mutex::new((1, Arc::new(snapshot))),
            hooks: Mutex::new(Arc::new(crate::hook_stats::dashboard_json_at(&places, now))),
            hooks_minute: AtomicI64::new(now.div_euclid(60)),
            clock,
            watches: Mutex::new(watches),
            integrity: Mutex::new(BTreeMap::new()),
            places,
            dirty: AtomicBool::new(false),
            wake: Condvar::new(),
            wake_lock: Mutex::new(()),
            listener: Mutex::new(None),
            stream: Mutex::new(None),
        });
        let weak = Arc::downgrade(&feed);
        let started = Stream::start_since(
            since,
            &[dir.as_path()],
            &[],
            0.1,
            Box::new(move |_| {
                if let Some(feed) = weak.upgrade() {
                    let _guard = feed.wake_lock.lock().expect("no panic holding it");
                    feed.dirty.store(true, Ordering::SeqCst);
                    feed.wake.notify_all();
                }
            }),
        );
        *feed.stream.lock().expect("no panic holding it") = Some(started);
        feed.check_integrity(&[]);
        let reader = feed.clone();
        std::thread::Builder::new()
            .name("builds-lists".into())
            .spawn(move || reader.read_loop())
            .expect("a thread starts");
        feed
    }

    /// Calls <listener> after a changed reading, with whether project lists changed.
    pub fn listen(&self, listener: Listener) {
        *self.listener.lock().expect("no panic holding it") = Some(listener);
    }

    /// The version and what the last read found.
    pub fn current(&self) -> (u64, Arc<Snapshot>) {
        let current = self.current.lock().expect("no panic holding it");
        (current.0, current.1.clone())
    }

    /// The hook runs' JSON as the last read built it. The same `Arc` comes
    /// back until a read finds the runs changed.
    pub fn hooks(&self) -> Arc<String> {
        self.hooks.lock().expect("no panic holding it").clone()
    }

    /// What the integrity checks found in every store's database, in project
    /// order, as the last check of each found it.
    pub fn integrity(&self) -> Vec<String> {
        self.integrity
            .lock()
            .expect("no panic holding it")
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    /// Checks again the integrity of each store whose database is among
    /// <moved> or was never checked, and forgets the stores that are gone.
    /// Returns whether any store's findings differ.
    fn check_integrity(&self, moved: &[PathBuf]) -> bool {
        let Ok(stores) = stores(&self.places) else {
            // No store is known to check; the read of the stores names why
            // their folder can't be listed.
            return self.clear_integrity();
        };
        let before = self.integrity.lock().expect("no panic holding it").clone();
        let checked: BTreeMap<String, Vec<String>> = stores
            .iter()
            .map(|store| {
                let found = match before.get(&store.project) {
                    Some(found) if !moved.contains(&store.database()) => found.clone(),
                    _ => store.integrity(),
                };
                (store.project.clone(), found)
            })
            .collect();
        let changed = checked != before;
        *self.integrity.lock().expect("no panic holding it") = checked;
        changed
    }

    fn clear_integrity(&self) -> bool {
        let mut held = self.integrity.lock().expect("no panic holding it");
        let had = !held.is_empty();
        held.clear();
        had
    }

    /// Reads after each burst of events, and at the start of each minute of
    /// the clock, when the hook runs' window moves.
    fn read_loop(self: Arc<Self>) {
        loop {
            {
                let guard = self.wake_lock.lock().expect("no panic holding it");
                let minute_ends = (self.clock)().rem_euclid(60);
                let _ = self
                    .wake
                    .wait_timeout_while(
                        guard,
                        Duration::from_secs((60 - minute_ends) as u64),
                        |_| !self.dirty.load(Ordering::SeqCst),
                    )
                    .expect("no panic holding it");
            }
            if self.dirty.load(Ordering::SeqCst) {
                // Debouncing: let a burst of writes finish.
                std::thread::sleep(SETTLE);
                self.dirty.store(false, Ordering::SeqCst);
            }
            self.read();
        }
    }

    /// When a database changed, reads the stores whose version moved and the
    /// hook runs again; in a new minute, reads the hook runs again. When
    /// either differs from the last read, it calls the listener, and it moves
    /// the version when the stores differ.
    fn read(&self) {
        let now = (self.clock)();
        let scan = scan_databases(
            &self.places,
            &mut self.watches.lock().expect("no panic holding it"),
        );
        let new_minute =
            self.hooks_minute.swap(now.div_euclid(60), Ordering::SeqCst) != now.div_euclid(60);
        let previous = self.current.lock().expect("no panic holding it").1.clone();
        let moved = matches!(&scan, Ok(scan) if !scan.moved.is_empty());
        let update = match &scan {
            Ok(scan) if moved || previous.is_err() => {
                let (read, changed) = reload(&self.places, &scan.paths, &previous);
                changed.then_some(read)
            }
            Ok(_) => None,
            Err(fault) => {
                let read = Err(fault.clone());
                (*previous != read).then_some(read)
            }
        };
        let stores_changed = update.is_some();
        let mut changed = stores_changed;
        if let Some(read) = update {
            report_snapshot_fault(&read, Some(&previous));
            let mut current = self.current.lock().expect("no panic holding it");
            *current = (current.0 + 1, Arc::new(read));
        }
        if changed || moved || new_minute {
            let hooks = crate::hook_stats::dashboard_json_at(&self.places, now);
            changed |= replace_if_changed(&self.hooks, hooks);
        }
        if changed {
            self.tell(stores_changed);
        }
        // After the lists and figures are out: the checks read every page
        // of the database.
        let integrity_changed = match scan {
            Ok(scan) if moved => self.check_integrity(&scan.moved),
            Ok(_) => false,
            Err(_) => self.clear_integrity(),
        };
        if integrity_changed {
            self.tell(false);
        }
    }

    /// Calls the listener, which says that something the feed serves changed.
    fn tell(&self, stores_changed: bool) {
        if let Some(listener) = &*self.listener.lock().expect("no panic holding it") {
            listener(stores_changed);
        }
    }
}

/// The discovered database paths and those whose watch changed.
struct DatabaseScan {
    paths: Vec<PathBuf>,
    moved: Vec<PathBuf>,
}

/// Reads the database paths and finds the watches that changed since the
/// last scan: added, removed, or moved. Keeps <watches> to the databases
/// there now. When the stores' folder can't be listed, drops
/// the watches and returns the fault for the feed to serve. A watch that can't be
/// opened or read leaves unknown whether its database changed: the log says
/// why, and the database counts as changed, so the feed reads the store again
/// and serves what is wrong with it.
fn scan_databases(
    places: &Places,
    watches: &mut HashMap<PathBuf, crate::db::Watch>,
) -> Result<DatabaseScan, String> {
    let paths = match database_paths(places) {
        Ok(paths) => paths,
        Err(error) => {
            watches.clear();
            return Err(error.text());
        }
    };
    let present: BTreeSet<&Path> = paths
        .iter()
        .map(PathBuf::as_path)
        .filter(|path| path.is_file())
        .collect();
    let mut moved: Vec<PathBuf> = watches
        .extract_if(|path, _| !present.contains(path.as_path()))
        .map(|(path, _)| path)
        .collect();
    for path in present {
        let changed = match watches.get_mut(path) {
            Some(watch) => watch.moved(),
            None => crate::db::Watch::open(path).map(|watch| {
                watches.insert(path.to_path_buf(), watch);
                true
            }),
        };
        match changed {
            Ok(false) => {}
            Ok(true) => moved.push(path.to_path_buf()),
            Err(error) => {
                crate::text::note(format!(
                    "telling whether {} changed: {}",
                    path.display(),
                    error.text()
                ));
                watches.remove(path);
                moved.push(path.to_path_buf());
            }
        }
    }
    Ok(DatabaseScan { paths, moved })
}

fn report_snapshot_fault(snapshot: &Snapshot, previous: Option<&Snapshot>) {
    let rows = match snapshot {
        Ok(rows) => rows,
        Err(fault) => {
            crate::text::note(format!("reading the project lists: {fault}"));
            return;
        }
    };
    let previous = previous.and_then(|snapshot| snapshot.as_ref().ok());
    for loaded in rows {
        if let Err(fault) = &loaded.lists
            && !previous.is_some_and(|previous| previous.contains(loaded))
        {
            crate::text::note(format!("reading the lists of {}: {fault}", loaded.project));
        }
    }
}

/// Puts <read> in <held> when it differs from what <held> holds, and returns
/// whether it did. An unchanged value keeps its `Arc`, which tells the page's
/// streams that nothing changed.
fn replace_if_changed(held: &Mutex<Arc<String>>, read: String) -> bool {
    let mut current = held.lock().expect("no panic holding it");
    if **current == read {
        return false;
    }
    *current = Arc::new(read);
    true
}

/// The worktrees of the running items of every project.
pub fn running_worktrees(snapshot: &Snapshot) -> Vec<String> {
    let Ok(stores) = snapshot.as_deref() else {
        return Vec::new();
    };
    stores
        .iter()
        .filter_map(|loaded| loaded.lists.as_ref().ok())
        .flat_map(|lists| &lists.items)
        .flat_map(|item| item.running.iter().map(|r| r.worktree.clone()))
        .collect()
}

/// The items running in <worktree>, in the order the items are listed.
fn items_in<'a>(
    lists: &'a crate::lists::Lists,
    worktree: &'a str,
) -> impl Iterator<Item = &'a crate::lists::Item> {
    lists
        .items
        .iter()
        .filter(move |item| item.running.iter().any(|r| r.worktree == worktree))
}

/// The items running in <worktree> as its card lists them, in the order the
/// items are listed.
pub fn running_items(
    lists: &crate::lists::Lists,
    worktree: &str,
) -> Vec<super::cards::RunningItem> {
    items_in(lists, worktree)
        .map(|item| super::cards::RunningItem {
            id: item.id.0,
            title: item.title.clone(),
        })
        .collect()
}

/// What the running items of <worktree> wait on that is not met, item by
/// item in list order. Fails when a wait names a record the lists do not
/// hold, or one of another kind.
pub fn running_waits(
    lists: &crate::lists::Lists,
    worktree: &str,
) -> Result<Vec<super::cards::ItemWait>, String> {
    let mut found = Vec::new();
    for item in items_in(lists, worktree) {
        found.extend(item_waits(lists, item)?);
    }
    Ok(found)
}

/// What <item> waits on that is not met, in the order of its waits. Fails
/// when a wait names a record the lists do not hold.
pub fn item_waits(
    lists: &crate::lists::Lists,
    item: &crate::lists::Item,
) -> Result<Vec<super::cards::ItemWait>, String> {
    use super::cards::ItemWait;
    use crate::lists::Wait;
    let missing =
        |id: crate::lists::Id| format!("{} waits on {id}, which the lists do not hold", item.id);
    // The title of the record a wait names: the open one, else the one that
    // closed.
    let closed_title = |id: crate::lists::Id| {
        lists
            .closed_record(id)
            .map(|closed| closed.record.title().to_string())
            .ok_or_else(|| missing(id))
    };
    let mut found: Vec<ItemWait> = Vec::new();
    for wait in crate::lists::derive::unmet_waits(lists, item) {
        found.push(match wait {
            Wait::Question(id) => ItemWait::Answer {
                id: id.0,
                title: match lists.question(*id) {
                    Some(question) => question.title.clone(),
                    None => closed_title(*id)?,
                },
            },
            Wait::Design(id) => {
                let title = match lists.design(*id) {
                    Some(design) => design.title.clone(),
                    None => closed_title(*id)?,
                };
                if crate::lists::derive::wait_on_person(lists, wait) {
                    ItemWait::Review { id: id.0, title }
                } else {
                    ItemWait::Landing { id: id.0, title }
                }
            }
            Wait::Item(id) => match (lists.item(*id), lists.closed_record(*id)) {
                (Some(other), _) => ItemWait::Item {
                    id: id.0,
                    title: other.title.clone(),
                    state: crate::lists::derive::item_state(lists, other).label(),
                },
                (None, Some(closed)) => ItemWait::Item {
                    id: id.0,
                    title: closed.record.title().to_string(),
                    state: closed.how.label(),
                },
                (None, None) => return Err(missing(*id)),
            },
            Wait::Condition { text, until } => ItemWait::Condition {
                text: text.clone(),
                until: *until,
            },
        });
    }
    Ok(found)
}

/// What the waits of <item>, which a worktree keeps, stand at as its card
/// says them.
pub fn kept_card(
    lists: &crate::lists::Lists,
    item: &crate::lists::Item,
) -> Result<super::cards::KeptCard, String> {
    use super::cards::KeptCard;
    use crate::lists::kept_work::{KeptWaits, kept_waits};
    Ok(
        match kept_waits(lists, item).map_err(crate::fail::Fail::text)? {
            KeptWaits::None => KeptCard::None,
            KeptWaits::Open => KeptCard::Open(item_waits(lists, item)?),
            KeptWaits::Met(met) => KeptCard::Met(met),
        },
    )
}

/// Open design change <id> of project <project>, and the change as the
/// lists JSON shows it, which fails on a broken store; None when that
/// project has no open design change with that id.
pub fn open_design(
    snapshot: &Snapshot,
    project: &str,
    id: u32,
) -> Option<(DesignChange, crate::fail::Result<DesignView>)> {
    let loaded = snapshot
        .as_deref()
        .ok()?
        .iter()
        .find(|loaded| loaded.project == project)?;
    let lists = loaded.lists.as_ref().ok()?;
    let design = lists.designs.iter().find(|d| d.id.0 == id)?;
    Some((design.clone(), design_view(lists, design, None)))
}

/// The repo that holds <lists>'s design files: the one repo its design
/// places' globs name. None when no repo has design places; fails when
/// several do, since the Designs page draws one repo's boards.
pub fn design_repo(lists: &crate::lists::Lists) -> crate::fail::Result<Option<&str>> {
    let repos: Vec<&str> = lists
        .design_places
        .iter()
        .filter(|(_, globs)| !globs.is_empty())
        .map(|(repo, _)| repo.as_str())
        .collect();
    match repos[..] {
        [] => Ok(None),
        [repo] => Ok(Some(repo)),
        _ => crate::fail!(
            "project {} keeps design files in {} repos ({}), and the Designs page draws the boards of one",
            lists.project,
            repos.len(),
            repos.join(", ")
        ),
    }
}

/// The repo that holds project <project>'s design files, as `design_repo`
/// finds it, with that repo's globs; None when the lists hold no such
/// project. A snapshot or project read failure returns the error.
pub fn project_design_repo(
    snapshot: &Snapshot,
    project: &str,
) -> Option<crate::fail::Result<Option<(String, Globs)>>> {
    let stores = match snapshot {
        Ok(stores) => stores,
        Err(fault) => return Some(Err(crate::fail::Fail::Message(fault.clone()))),
    };
    let loaded = stores.iter().find(|loaded| loaded.project == project)?;
    Some(match &loaded.lists {
        Ok(lists) => design_repo(lists)
            .map(|repo| repo.map(|repo| (repo.to_string(), Globs::of(lists, repo)))),
        Err(fault) => Err(crate::fail::Fail::Message(fault.clone())),
    })
}

/// What the page reads: one lists JSON per project, in project order. No
/// project, or a store that fails its check, is that project's `error`.
/// <claims> answers for a running item's worktree, and <reviews> for an open
/// design change of a project, by its id, that waits on the person's
/// approval.
pub fn compose(
    snapshot: &Snapshot,
    now: i64,
    claims: &dyn Fn(&str) -> ClaimFacts,
    reviews: &Reviews,
) -> Vec<Composed> {
    match snapshot.as_deref() {
        Ok([]) => vec![Composed::of(
            View::failed(
                "",
                0,
                "there are no lists; `builds lists init <project>` makes them".to_string(),
            ),
            false,
        )],
        Ok(stores) => stores
            .iter()
            .map(|loaded| {
                let designs = loaded
                    .lists
                    .as_ref()
                    .is_ok_and(|lists| !matches!(design_repo(lists), Ok(None)));
                Composed::of(compose_view(loaded, now, claims, reviews), designs)
            })
            .collect(),
        Err(fault) => vec![Composed::of(View::failed("", 0, fault.clone()), false)],
    }
}

/// The lists JSON of every project, as the page and `/api/lists` have it.
#[derive(serde::Serialize)]
pub struct Served<'a> {
    pub projects: &'a [Composed],
}

/// One project's lists JSON: the view `lists show --json` prints, and the
/// running work the Work tab lists (board.rs), which needs the claim facts
/// only the daemon has.
#[derive(serde::Serialize)]
pub struct Composed {
    #[serde(flatten)]
    pub view: View,
    pub works: Vec<super::board::Work>,
    pub board: super::board::Board,
    /// Whether the project has a Designs page: a repo holds its design files.
    /// The view's `designs` is the list of its design changes. A project whose design files are in several repos has one too, which
    /// says why it draws none (`design_repo`).
    pub has_designs: bool,
}

impl Composed {
    fn of(view: View, has_designs: bool) -> Composed {
        let works = super::board::works(&view);
        let board = super::board::board(&view);
        Composed {
            view,
            works,
            board,
            has_designs,
        }
    }
}

/// Where the rules review of a project's open design change, by its id,
/// stands, as the daemon reads it from the change's worktree.
pub type Reviews<'a> = dyn Fn(&str, Id) -> RulesReview + 'a;

fn compose_view(
    loaded: &Loaded,
    now: i64,
    claims: &dyn Fn(&str) -> ClaimFacts,
    reviews: &Reviews,
) -> View {
    match &loaded.lists {
        Err(fault) => View::failed(&loaded.project, 0, fault.clone()),
        Ok(lists) => {
            let mut view = match View::build(lists, now) {
                Ok(view) => view,
                Err(fault) => return View::failed(&loaded.project, lists.version, fault.text()),
            };
            for item in &mut view.items {
                for running in &mut item.running {
                    running.claim = Some(claims(&running.worktree));
                }
            }
            for design in &mut view.designs {
                if design.closed.is_none() && design.approved.is_none() {
                    design.rules_review = Some(reviews(&lists.project, design.id));
                }
            }
            view
        }
    }
}

/// Notifies the dashboard when a feed changes, with whether the stores moved.
type Listener = Box<dyn Fn(bool) + Send + Sync>;

#[cfg(test)]
#[path = "lists_tests.rs"]
mod tests;
