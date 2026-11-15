//! The lists as the page reads them: the store's folder followed live, and
//! the lists JSON built from what was read and each
//! running item's worktree.
//!
//! `Feed` follows `Places::lists` with an FSEvents stream and, after the
//! settle delay, asks each store for its last write's version: a store that
//! has not moved is not read again (a hook run's write wakes the feed and
//! reads nothing more), and the reads that found a store changed are counted.
//! Each project's hook runs are read again too. A store that fails its check
//! is served as the JSON's `error`; nothing repairs it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use super::fsevents::Stream;
use super::model::SETTLE;
use crate::lists::DesignChange;
use crate::lists::store::{Loaded, load_all, reload};
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
    /// The time the hook runs' 12-hour window ends at, in seconds since
    /// 1970; the wall clock, except in a test that sets it.
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
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
        let feed = Arc::new(Feed {
            current: Mutex::new((1, Arc::new(load_all(&places)))),
            hooks: Mutex::new(Arc::new(crate::hook_stats::dashboard_json_at(
                &places,
                clock(),
            ))),
            clock,
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

    fn read_loop(self: Arc<Self>) {
        loop {
            {
                let mut guard = self.wake_lock.lock().expect("no panic holding it");
                while !self.dirty.load(Ordering::SeqCst) {
                    guard = self.wake.wait(guard).expect("no panic holding it");
                }
            }
            // Debouncing: let a burst of writes finish.
            std::thread::sleep(SETTLE);
            self.dirty.store(false, Ordering::SeqCst);
            self.read();
        }
    }

    /// Reads the stores whose version moved and the hook runs again; when
    /// either differs from the last read, the listener is called, and the
    /// version moves when the stores differ.
    fn read(&self) {
        let previous = self.current.lock().expect("no panic holding it").1.clone();
        let (read, stores_changed) = reload(&self.places, &previous);
        let hooks = crate::hook_stats::dashboard_json_at(&self.places, (self.clock)());
        let mut changed = false;
        if stores_changed {
            let mut current = self.current.lock().expect("no panic holding it");
            *current = (current.0 + 1, Arc::new(read));
            changed = true;
        }
        {
            let mut current = self.hooks.lock().expect("no panic holding it");
            if **current != hooks {
                *current = Arc::new(hooks);
                changed = true;
            }
        }
        if changed && let Some(listener) = &*self.listener.lock().expect("no panic holding it") {
            listener();
        }
    }
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
fn running_items<'a>(
    lists: &'a crate::lists::Lists,
    worktree: &'a str,
) -> impl Iterator<Item = &'a crate::lists::Item> {
    lists
        .items
        .iter()
        .filter(move |item| item.running.iter().any(|r| r.worktree == worktree))
}

/// The titles of the items running in <worktree>, in the order the items
/// are listed.
pub fn running_titles(lists: &crate::lists::Lists, worktree: &str) -> Vec<String> {
    running_items(lists, worktree)
        .map(|item| item.title.clone())
        .collect()
}

/// The numbers of the items running in <worktree>, in the order the items
/// are listed.
pub fn running_numbers(lists: &crate::lists::Lists, worktree: &str) -> Vec<u32> {
    running_items(lists, worktree)
        .map(|item| item.id.0)
        .collect()
}

/// What the running items of <worktree> wait on that is not met, the first
/// that needs the person (a question, a design change), else an item.
pub fn running_wait(lists: &crate::lists::Lists, worktree: &str) -> Option<super::cards::ItemWait> {
    use super::cards::ItemWait;
    use crate::lists::Wait;
    let mut found: Vec<ItemWait> = Vec::new();
    for item in lists
        .items
        .iter()
        .filter(|item| item.running.iter().any(|r| r.worktree == worktree))
    {
        for wait in crate::lists::derive::unmet_waits(lists, item) {
            let title = |id| lists.locate(id).map(|found| found.title.to_string());
            found.extend(match wait {
                Wait::Question(id) => title(*id).map(ItemWait::Answer),
                Wait::Design(id) => title(*id).map(|title| ItemWait::Review { id: id.0, title }),
                Wait::Item(id) => title(*id).map(|title| ItemWait::Item { id: id.0, title }),
                Wait::Condition(_) => None,
            });
        }
    }
    found.sort_by_key(ItemWait::rank);
    found.into_iter().next()
}

/// Open design change <id> of project <project>, as the lists JSON shows it;
/// None when that project has no open design change with that id.
pub fn open_design(
    snapshot: &Snapshot,
    project: &str,
    id: u32,
) -> Option<(DesignChange, DesignView)> {
    let loaded = snapshot
        .as_deref()
        .ok()?
        .iter()
        .find(|loaded| loaded.project == project)?;
    let lists = loaded.lists.as_ref().ok()?;
    let design = lists.designs.iter().find(|d| d.id.0 == id)?;
    Some((design.clone(), design_view(lists, design, None)))
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
        Ok([]) => vec![Composed::of(View::failed(
            "",
            0,
            "there are no lists; `builds lists init <project>` makes them".to_string(),
        ))],
        Ok(stores) => stores
            .iter()
            .map(|loaded| Composed::of(compose_view(loaded, now, claims)))
            .collect(),
        Err(fault) => vec![Composed::of(View::failed("", 0, fault.clone()))],
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
}

impl Composed {
    fn of(view: View) -> Composed {
        let works = super::board::works(&view);
        let board = super::board::board(&view);
        Composed { view, works, board }
    }
}

fn compose_view(loaded: &Loaded, now: i64, claims: &dyn Fn(&str) -> ClaimFacts) -> View {
    match &loaded.lists {
        Err(fault) => View::failed(&loaded.project, 0, fault.clone()),
        Ok(lists) => {
            let mut view = View::build(lists, now);
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
