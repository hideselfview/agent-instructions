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
use crate::lists::store::{Loaded, load_all, reload};
use crate::lists::DesignChange;
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
        let dir: PathBuf = places.lists.clone();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("builds: creating {}: {error}", dir.display());
        }
        let feed = Arc::new(Feed {
            current: Mutex::new((1, Arc::new(load_all(&places)))),
            hooks: Mutex::new(Arc::new(crate::hook_stats::dashboard_json(&places))),
            places,
            dirty: AtomicBool::new(false),
            wake: Condvar::new(),
            wake_lock: Mutex::new(()),
            listener: Mutex::new(None),
            stream: Mutex::new(None),
        });
        let weak = Arc::downgrade(&feed);
        let started = Stream::start(
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
        match started {
            Some(started) => *feed.stream.lock().expect("no panic holding it") = Some(started),
            None => eprintln!("builds: FSEvents would not follow {}", dir.display()),
        }
        let reader = feed.clone();
        std::thread::Builder::new().name("builds-lists".into()).spawn(move || reader.read_loop()).expect("a thread starts");
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
        let hooks = crate::hook_stats::dashboard_json(&self.places);
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

/// The worktrees of the running items of the one project there is.
pub fn running_worktrees(snapshot: &Snapshot) -> Vec<String> {
    match snapshot.as_deref() {
        Ok([Loaded { lists: Ok(lists), .. }]) => {
            lists.items.iter().flat_map(|item| item.running.iter().map(|r| r.worktree.clone())).collect()
        }
        _ => Vec::new(),
    }
}

/// Open design change <id> of the one project there is, as the lists JSON
/// shows it; None when no open design change has that id.
pub fn open_design(snapshot: &Snapshot, id: u32) -> Option<(DesignChange, DesignView)> {
    let Ok([Loaded { lists: Ok(lists), .. }]) = snapshot.as_deref() else { return None };
    let design = lists.designs.iter().find(|d| d.id.0 == id)?;
    Some((design.clone(), design_view(lists, design, None)))
}

/// The view the page reads, for the one project there is. No project, or
/// several (the sidebar shows one), is the `error`, as is a store that fails
/// its check. <claims> answers for a running item's worktree.
pub fn compose(snapshot: &Snapshot, now: i64, claims: &dyn Fn(&str) -> ClaimFacts) -> View {
    let loaded = match snapshot.as_deref() {
        Ok([one]) => one,
        Ok([]) => {
            return View::failed("", 0, "there are no lists; `builds lists init <project>` makes them".to_string());
        }
        Ok(several) => {
            let names: Vec<&str> = several.iter().map(|l| l.project.as_str()).collect();
            return View::failed("", 0, format!("there are several projects ({}) and the sidebar shows one", names.join(", ")));
        }
        Err(fault) => return View::failed("", 0, fault.clone()),
    };
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
