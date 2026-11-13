//! The lists as the page reads them: the store's folder followed live, and
//! the lists JSON built from what was read, the machine's mode and each
//! running item's worktree.
//!
//! `Feed` follows `Places::lists` with an FSEvents stream, reads every store
//! again after the settle delay, and counts the reads that found something
//! new in its own `version`. A store that fails its check is served as the
//! JSON's `error`; nothing repairs it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use super::fsevents::Stream;
use super::model::SETTLE;
use crate::lists::store::{Loaded, load_all};
use crate::lists::view::{ClaimFacts, View};
use crate::places::Places;

type Snapshot = Result<Vec<Loaded>, String>;

pub struct Feed {
    places: Places,
    /// The version counts reads that changed what is served; the snapshot is
    /// what the last of them read.
    current: Mutex<(u64, Arc<Snapshot>)>,
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

    /// Reads the stores again; when they differ from the last read, the
    /// version moves and the listener is called.
    fn read(&self) {
        let read = load_all(&self.places);
        {
            let mut current = self.current.lock().expect("no panic holding it");
            if *current.1 == read {
                return;
            }
            *current = (current.0 + 1, Arc::new(read));
        }
        if let Some(listener) = &*self.listener.lock().expect("no panic holding it") {
            listener();
        }
    }
}

/// The worktrees of the running items of the one project there is.
pub fn running_worktrees(snapshot: &Snapshot) -> Vec<String> {
    match snapshot.as_deref() {
        Ok([Loaded { lists: Ok(lists), .. }]) => {
            lists.items.iter().filter_map(|item| item.running.as_ref().map(|r| r.worktree.clone())).collect()
        }
        _ => Vec::new(),
    }
}

/// The image file of the <n>th board (by path) of open design change <id> of
/// the one project there is, with its content type; None when no such board
/// has a render, or the file's type is not one of the image types a render
/// may be.
pub fn design_render(snapshot: &Snapshot, id: u32, n: usize) -> Option<(String, &'static str)> {
    let Ok([Loaded { lists: Ok(lists), .. }]) = snapshot.as_deref() else { return None };
    let design = lists.designs.iter().find(|d| d.id.0 == id)?;
    let render = design.boards.values().nth(n)?.render.clone()?;
    let kind = match std::path::Path::new(&render).extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => return None,
    };
    Some((render, kind))
}

/// The view the page reads, for the one project there is. No project, or
/// several (the sidebar shows one), is the `error`, as is a store that fails
/// its check. <claims> answers for a running item's worktree.
pub fn compose(snapshot: &Snapshot, now: i64, mode: &str, claims: &dyn Fn(&str) -> ClaimFacts) -> View {
    let loaded = match snapshot.as_deref() {
        Ok([one]) => one,
        Ok([]) => {
            return View::failed("", 0, "there are no lists; `builds lists init <project>` makes them".to_string(), mode);
        }
        Ok(several) => {
            let names: Vec<&str> = several.iter().map(|l| l.project.as_str()).collect();
            return View::failed("", 0, format!("there are several projects ({}) and the sidebar shows one", names.join(", ")), mode);
        }
        Err(fault) => return View::failed("", 0, fault.clone(), mode),
    };
    match &loaded.lists {
        Err(fault) => View::failed(&loaded.project, 0, fault.clone(), mode),
        Ok(lists) => {
            let mut view = View::build(lists, now, mode);
            for item in &mut view.items {
                if let Some(running) = &mut item.running {
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
