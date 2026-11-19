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

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::fsevents::Stream;
use super::model::SETTLE;
use crate::lists::DesignChange;
use crate::lists::store::{Loaded, database_paths, load_all, reload, stores};
use crate::lists::view::{ClaimFacts, DesignView, View, design_view};
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
    listener: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
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
        databases_moved(&places, &mut watches);
        let now = clock();
        let feed = Arc::new(Feed {
            current: Mutex::new((1, Arc::new(load_all(&places)))),
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

    /// Calls <listener> after each read that found something new.
    pub fn listen(&self, listener: Box<dyn Fn() + Send + Sync>) {
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
            let mut held = self.integrity.lock().expect("no panic holding it");
            let had = !held.is_empty();
            held.clear();
            return had;
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
        let moved_databases = databases_moved(
            &self.places,
            &mut self.watches.lock().expect("no panic holding it"),
        );
        let moved = !moved_databases.is_empty();
        let new_minute =
            self.hooks_minute.swap(now.div_euclid(60), Ordering::SeqCst) != now.div_euclid(60);
        let mut changed = false;
        if moved {
            let previous = self.current.lock().expect("no panic holding it").1.clone();
            let (read, stores_changed) = reload(&self.places, &previous);
            if stores_changed {
                let mut current = self.current.lock().expect("no panic holding it");
                *current = (current.0 + 1, Arc::new(read));
                changed = true;
            }
        }
        if moved || new_minute {
            let hooks = crate::hook_stats::dashboard_json_at(&self.places, now);
            changed |= replace_if_changed(&self.hooks, hooks);
        }
        if changed {
            self.tell();
        }
        // After the lists and figures are out: the checks read every page
        // of the database.
        if moved && self.check_integrity(&moved_databases) {
            self.tell();
        }
    }

    /// Calls the listener, which says that something the feed serves changed.
    fn tell(&self) {
        if let Some(listener) = &*self.listener.lock().expect("no panic holding it") {
            listener();
        }
    }
}

/// The databases of <places>'s stores that changed since <watches> last
/// looked: added, removed, or moved as their watch says. Keeps <watches> to
/// the databases there now. When the stores' folder can't be listed, the
/// daemon's log says why and every watched database counts as changed, so the
/// feed reads the stores again and serves the fault. A watch that can't be
/// opened or read leaves unknown whether its database changed: the log says
/// why, and the database counts as changed, so the feed reads the store again
/// and serves what is wrong with it.
fn databases_moved(
    places: &Places,
    watches: &mut HashMap<PathBuf, crate::db::Watch>,
) -> Vec<PathBuf> {
    let databases: Vec<PathBuf> = match database_paths(places) {
        Ok(paths) => paths.into_iter().filter(|path| path.is_file()).collect(),
        Err(error) => {
            crate::text::note(format!("following the stores: {}", error.text()));
            return watches.drain().map(|(path, _)| path).collect();
        }
    };
    let mut moved: Vec<PathBuf> = watches
        .keys()
        .filter(|path| !databases.contains(path))
        .cloned()
        .collect();
    watches.retain(|path, _| databases.contains(path));
    for path in databases {
        let changed = match watches.get_mut(&path) {
            Some(watch) => watch.moved(),
            None => crate::db::Watch::open(&path).map(|watch| {
                watches.insert(path.clone(), watch);
                true
            }),
        };
        match changed {
            Ok(false) => {}
            Ok(true) => moved.push(path),
            Err(error) => {
                crate::text::note(format!(
                    "telling whether {} changed: {}",
                    path.display(),
                    error.text()
                ));
                watches.remove(&path);
                moved.push(path);
            }
        }
    }
    moved
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
/// finds it; None when the lists hold no such project.
pub fn project_design_repo(
    snapshot: &Snapshot,
    project: &str,
) -> Option<crate::fail::Result<Option<String>>> {
    let loaded = snapshot
        .as_deref()
        .ok()?
        .iter()
        .find(|loaded| loaded.project == project)?;
    Some(match &loaded.lists {
        Ok(lists) => design_repo(lists).map(|repo| repo.map(str::to_string)),
        Err(fault) => Err(crate::fail::Fail::Message(fault.clone())),
    })
}

/// What the page reads: one lists JSON per project, in project order. No
/// project, or a store that fails its check, is that project's `error`.
/// <claims> answers for a running item's worktree.
pub fn compose(
    snapshot: &Snapshot,
    now: i64,
    claims: &dyn Fn(&str) -> ClaimFacts,
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
                Composed::of(compose_view(loaded, now, claims), designs)
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

fn compose_view(loaded: &Loaded, now: i64, claims: &dyn Fn(&str) -> ClaimFacts) -> View {
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
            view
        }
    }
}

#[cfg(test)]
#[path = "lists_tests.rs"]
mod tests;
