//! Keeps the index of the build volume current: a full walk when the daemon
//! starts or the volume comes back, then every change FSEvents reports.
//!
//! - The stream on the root starts before the walk; events that arrive
//!   during it wait and are applied after it.
//! - The stream's callback only queues its events; one thread applies them,
//!   everything queued at once. A callback that walked or read the disk
//!   itself would hold up delivery, and FSEvents drops events for a client
//!   that falls behind, which asks for a walk of the whole root: under load
//!   40 that came back every few seconds, each walk causing the next.
//! - Applying an event never trusts the event's flags for what a path is now:
//!   it reads the path. Gone: its entry and everything under it go. A file:
//!   its facts are read again. A folder that is new or has a new inode is
//!   walked, or a folder removed in the same batch with that inode is put
//!   back under its new name (a rename, such as an eviction into .trash,
//!   moves 100k entries without a walk). The parent of each path is read too,
//!   since a deletion writes only to its folder.
//! - The root's macOS entries and its trash are not followed (see
//!   `places::unfollowed`); a question about them gets no answer.
//! - MUST_SCAN_SUBDIRS on a path walks that subtree again; a dropped-events
//!   flag walks the whole root again. While a walk runs, a question about the
//!   folders it covers gets no answer, and the asker measures them itself.
//! - The root changing or its volume unmounting stops the stream; a stream
//!   on the root's parent reports it coming back, and then the stream starts
//!   again and the root is walked. Nothing polls.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use super::fsevents::{Detail, Event, Stream, flags};
use super::index::{Entry, Index, KindOf, NodeId, gone, walk};
use crate::usage::Measure;

const WORKERS: usize = 8;

#[derive(Default)]
struct Flags {
    /// The index holds the whole root.
    ready: bool,
    /// Events that arrived while the index was being walked.
    pending: Vec<Event>,
    /// Subtrees being walked again.
    rescanning: Vec<PathBuf>,
    /// The root's (dev, inode) when its stream started.
    followed: Option<(u64, u64)>,
    /// The root's stream needs starting again.
    restart: bool,
    /// How many times the root has been walked whole.
    walks: u64,
}

/// The marks questions write, and how far the stream has delivered them.
#[derive(Default)]
struct Marks {
    /// The last mark a question took.
    taken: u64,
    /// The highest mark whose event has been handled.
    seen: u64,
}

/// Told the paths each handled batch named; after a whole walk, the root.
pub type Listener = Box<dyn Fn(&[PathBuf]) + Send + Sync>;

pub struct Tracker {
    pub root: PathBuf,
    listener: Mutex<Option<Listener>>,
    /// Where questions write their marks: a file `mark-<n>` whose event,
    /// once handled, says every write made before the question has been.
    marks_dir: PathBuf,
    marks: Mutex<Marks>,
    delivered: Condvar,
    index: Mutex<Index>,
    flags: Mutex<Flags>,
    wake: Condvar,
    stream: Mutex<Option<Stream>>,
    /// Events delivered and not yet applied.
    queue: Mutex<Vec<Event>>,
    queued: Condvar,
    /// Follows the root's parent, to learn the root came back.
    _parent: Option<Stream>,
}

/// One folder's answer: its size and newest write, or None when the daemon
/// can't say (still walking it, or outside the root).
pub type Answer = Option<Measure>;

impl Tracker {
    /// Starts following <root> (an absolute, resolved path).
    pub fn start(root: &Path, marks_dir: &Path) -> std::io::Result<Arc<Tracker>> {
        let meta = std::fs::symlink_metadata(root)?;
        std::fs::create_dir_all(marks_dir)?;
        let tracker = Arc::new_cyclic(|me: &std::sync::Weak<Tracker>| {
            let weak = me.clone();
            let parent = root.parent().and_then(|parent| {
                Stream::start(
                    &[parent],
                    &[],
                    Detail::Folders,
                    0.5,
                    Box::new(move |_| {
                        if let Some(tracker) = weak.upgrade() {
                            tracker.parent_changed();
                        }
                    }),
                )
            });
            Tracker {
                root: root.to_path_buf(),
                listener: Mutex::new(None),
                marks_dir: marks_dir.to_path_buf(),
                marks: Mutex::new(Marks::default()),
                delivered: Condvar::new(),
                index: Mutex::new(Index::new(root, &meta)),
                flags: Mutex::new(Flags { restart: true, ..Flags::default() }),
                wake: Condvar::new(),
                stream: Mutex::new(None),
                queue: Mutex::new(Vec::new()),
                queued: Condvar::new(),
                _parent: parent,
            }
        });
        let supervisor = tracker.clone();
        std::thread::Builder::new()
            .name("builds-tracker".into())
            .spawn(move || supervisor.supervise())?;
        let applier = tracker.clone();
        std::thread::Builder::new()
            .name("builds-apply".into())
            .spawn(move || applier.apply_loop())?;
        Ok(tracker)
    }

    fn signal_restart(&self, why: &str) {
        eprintln!("builds: {} {why}; following it again", self.root.display());
        let mut flags = self.flags.lock().expect("no panic holding it");
        flags.restart = true;
        flags.ready = false;
        flags.followed = None;
        self.wake.notify_all();
    }

    /// Something changed beside the root: it may have come back, or been
    /// replaced.
    fn parent_changed(&self) {
        let followed = self.flags.lock().expect("no panic holding it").followed;
        let back = followed.is_none() && self.root.is_dir();
        if back {
            self.signal_restart("came back");
        } else if followed.is_some() && self.root_replaced() {
            self.signal_restart("was replaced");
        }
    }

    /// Starts the root's stream and walks the root whenever a restart is
    /// due and the root is there.
    fn supervise(self: Arc<Self>) {
        loop {
            {
                let mut flags = self.flags.lock().expect("no panic holding it");
                while !flags.restart {
                    flags = self.wake.wait(flags).expect("no panic holding it");
                }
                flags.restart = false;
                flags.ready = false;
                flags.pending.clear();
            }
            *self.stream.lock().expect("no panic holding it") = None;
            self.flags.lock().expect("no panic holding it").followed = None;
            let Ok(meta) = std::fs::symlink_metadata(&self.root) else {
                eprintln!("builds: {} is gone; waiting for it to come back", self.root.display());
                continue;
            };
            *self.index.lock().expect("no panic holding it") = Index::new(&self.root, &meta);
            self.flags.lock().expect("no panic holding it").followed = Some((meta.dev(), meta.ino()));
            let me = Arc::downgrade(&self);
            let trash = self.root.join(".trash");
            let stream = Stream::start(
                &[self.root.as_path()],
                &[trash.as_path()],
                Detail::Files,
                0.2,
                Box::new(move |events| {
                    if let Some(tracker) = me.upgrade() {
                        tracker.queue.lock().expect("no panic holding it").extend(events);
                        tracker.queued.notify_all();
                    }
                }),
            );
            if stream.is_none() {
                eprintln!("builds: FSEvents would not follow {}", self.root.display());
                continue;
            }
            *self.stream.lock().expect("no panic holding it") = stream;
            let started = std::time::Instant::now();
            walk(&self.index, 0, &self.root, WORKERS);
            loop {
                let pending = {
                    let mut flags = self.flags.lock().expect("no panic holding it");
                    if flags.restart {
                        break;
                    }
                    if flags.pending.is_empty() {
                        flags.ready = true;
                        flags.walks += 1;
                        self.wake.notify_all();
                        break;
                    }
                    std::mem::take(&mut flags.pending)
                };
                self.apply(pending);
            }
            self.tell(std::slice::from_ref(&self.root));
            let (entries, bytes) = self.size();
            eprintln!(
                "builds: following {}: {entries} entries, walked in {:.1}s, {} in memory",
                self.root.display(),
                started.elapsed().as_secs_f64(),
                crate::text::human_bytes(bytes as u64)
            );
        }
    }

    /// Has <listener> told of every batch handled from now on.
    pub fn listen(&self, listener: Listener) {
        *self.listener.lock().expect("no panic holding it") = Some(listener);
    }

    fn tell(&self, paths: &[PathBuf]) {
        if let Some(listener) = self.listener.lock().expect("no panic holding it").as_ref() {
            listener(paths);
        }
    }

    /// Is the root no longer the folder its stream started on?
    fn root_replaced(&self) -> bool {
        let followed = self.flags.lock().expect("no panic holding it").followed;
        let now = std::fs::symlink_metadata(&self.root).ok().map(|meta| (meta.dev(), meta.ino()));
        now.is_none() || now != followed
    }

    /// Applies the queued events, everything queued at once, for as long
    /// as the daemon runs.
    fn apply_loop(&self) {
        loop {
            let events = {
                let mut queue = self.queue.lock().expect("no panic holding it");
                while queue.is_empty() {
                    queue = self.queued.wait(queue).expect("no panic holding it");
                }
                std::mem::take(&mut *queue)
            };
            self.on_events(events);
        }
    }

    fn on_events(&self, events: Vec<Event>) {
        // Marks are entries like any other; their events also say how far
        // delivery has come.
        let mark = events
            .iter()
            .filter(|event| event.path.parent() == Some(self.marks_dir.as_path()))
            .filter_map(|event| event.path.file_name()?.to_str()?.strip_prefix("mark-")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        let paths: Vec<PathBuf> = events.iter().map(|event| event.path.clone()).collect();
        self.handle(events);
        self.tell(&paths);
        if mark > 0 {
            let mut marks = self.marks.lock().expect("no panic holding it");
            marks.seen = marks.seen.max(mark);
            self.delivered.notify_all();
        }
    }

    fn handle(&self, events: Vec<Event>) {
        // The stream also says ROOT_CHANGED when a folder above the root
        // changes, the root itself staying where it was.
        let root_changed = events.iter().any(|event| {
            event.flags & (flags::ROOT_CHANGED | flags::UNMOUNT) != 0 && event.path.starts_with(&self.root)
        });
        if root_changed && self.root_replaced() {
            self.signal_restart("moved or unmounted");
            return;
        }
        {
            let mut flags = self.flags.lock().expect("no panic holding it");
            if flags.restart {
                return;
            }
            if !flags.ready {
                flags.pending.extend(events);
                return;
            }
        }
        let dropped = events
            .iter()
            .map(|event| event.flags & (flags::USER_DROPPED | flags::KERNEL_DROPPED | flags::EVENT_IDS_WRAPPED))
            .fold(0, |all, flags| all | flags);
        if dropped != 0 {
            // Every other event in the batch came before the walk, which
            // reads what they changed.
            eprintln!("builds: FSEvents dropped events (flags {dropped:#x})");
            self.rescan(&self.root.clone());
            return;
        }
        let (scans, others): (Vec<Event>, Vec<Event>) =
            events.into_iter().partition(|event| event.flags & flags::MUST_SCAN_SUBDIRS != 0);
        self.apply(others);
        for event in scans {
            self.rescan(&event.path);
        }
    }

    /// Walks <path> again in place, answering nothing about it meanwhile.
    fn rescan(&self, path: &Path) {
        eprintln!("builds: FSEvents asked for {} to be read again", path.display());
        self.apply(vec![Event { path: path.to_path_buf(), flags: 0 }]);
        let id = self.index.lock().expect("no panic holding it").lookup(path);
        let Some(id) = id else { return };
        self.flags.lock().expect("no panic holding it").rescanning.push(path.to_path_buf());
        walk(&self.index, id, path, WORKERS);
        self.flags.lock().expect("no panic holding it").rescanning.retain(|p| p != path);
    }

    /// Is <path> in one of the root's entries the index leaves out?
    fn left_out(&self, path: &Path) -> bool {
        let Ok(rest) = path.strip_prefix(&self.root) else { return false };
        let first = rest.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned());
        first.is_some_and(|name| crate::places::unfollowed(&name))
    }

    /// Reads each changed path and its folder, and records what is there now.
    fn apply(&self, events: Vec<Event>) {
        let mut paths: BTreeMap<PathBuf, Option<std::fs::Metadata>> = BTreeMap::new();
        for event in events {
            if !event.path.starts_with(&self.root) || self.left_out(&event.path) {
                continue;
            }
            if let Some(parent) = event.path.parent().filter(|p| p.starts_with(&self.root)) {
                paths.entry(parent.to_path_buf()).or_insert(None);
            }
            paths.entry(event.path).or_insert(None);
        }
        let mut read: Vec<(PathBuf, Option<std::fs::Metadata>)> = Vec::new();
        for path in paths.into_keys() {
            match std::fs::symlink_metadata(&path) {
                Ok(meta) => read.push((path, Some(meta))),
                Err(error) if gone(&error) => read.push((path, None)),
                Err(error) => eprintln!("builds: reading {}: {error}; left as it was", path.display()),
            }
        }
        let mut walks: Vec<(NodeId, PathBuf)> = Vec::new();
        {
            let mut index = self.index.lock().expect("no panic holding it");
            // Gone first, so a folder renamed within the batch is found again
            // by its inode wherever the batch lists it.
            let mut limbo: HashMap<u64, NodeId> = HashMap::new();
            for (path, meta) in &read {
                if meta.is_some() {
                    continue;
                }
                if let Some(id) = index.lookup(path) {
                    if index.is_dir(id) {
                        index.detach(id);
                        limbo.insert(index.ino(id), id);
                    } else {
                        index.remove(id);
                    }
                }
            }
            // Parents before children: the map is sorted by path.
            for (path, meta) in &read {
                let Some(meta) = meta else { continue };
                let name = path.file_name().map(|n| n.as_encoded_bytes().to_vec()).unwrap_or_default();
                let entry = Entry::read(&name, path, meta);
                if *path == self.root {
                    index.set_root(&entry);
                    continue;
                }
                if walks.iter().any(|(_, walked)| path.starts_with(walked)) {
                    continue;
                }
                let parent_path = path.parent().expect("a path below the root has a parent");
                let Some(parent) = index.lookup(parent_path) else {
                    // A folder the index lost track of: walk from the first
                    // one missing.
                    let mut missing = path.as_path();
                    while let Some(up) = missing.parent() {
                        if index.lookup(up).is_some() {
                            break;
                        }
                        missing = up;
                    }
                    if let Ok(meta) = std::fs::symlink_metadata(missing) {
                        let up = index.lookup(missing.parent().expect("below the root")).expect("found above");
                        let name = missing.file_name().map(|n| n.as_encoded_bytes().to_vec()).unwrap_or_default();
                        let id = index.set(up, Entry::read(&name, missing, &meta));
                        if meta.is_dir() {
                            walks.push((id, missing.to_path_buf()));
                        }
                    }
                    continue;
                };
                if !index.is_dir(parent) {
                    continue;
                }
                if entry.kind == KindOf::Dir {
                    let existing = index.lookup(path).filter(|id| index.is_dir(*id) && index.ino(*id) == entry.ino);
                    if existing.is_some() {
                        index.set(parent, entry);
                    } else if let Some(moved) = limbo.remove(&entry.ino) {
                        index.attach(moved, parent, &name);
                        index.set(parent, entry);
                    } else {
                        let id = index.set(parent, entry);
                        walks.push((id, path.clone()));
                    }
                } else {
                    index.set(parent, entry);
                }
            }
            for (_, id) in limbo {
                index.discard(id);
            }
        }
        for (id, path) in walks {
            walk(&self.index, id, &path, WORKERS);
        }
    }

    /// Waits until the stream has delivered every write made before the
    /// call: writes a mark and waits for its event. False when that took
    /// longer than a question may wait, or the mark could not be written.
    fn catch_up(&self) -> bool {
        let n = {
            let mut marks = self.marks.lock().expect("no panic holding it");
            marks.taken += 1;
            marks.taken
        };
        let mark = self.marks_dir.join(format!("mark-{n}"));
        if let Err(error) = std::fs::write(&mark, b"") {
            eprintln!("builds: writing {}: {error}; answering nothing", mark.display());
            return false;
        }
        if let Some(stream) = self.stream.lock().expect("no panic holding it").as_ref() {
            stream.flush();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut marks = self.marks.lock().expect("no panic holding it");
        let caught_up = loop {
            if marks.seen >= n {
                break true;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                eprintln!("builds: FSEvents did not deliver {} in 10s; answering nothing", mark.display());
                break false;
            }
            marks = self.delivered.wait_timeout(marks, left).expect("no panic holding it").0;
        };
        drop(marks);
        if let Err(error) = std::fs::remove_file(&mark) {
            eprintln!("builds: removing {}: {error}", mark.display());
        }
        caught_up
    }

    /// The size and newest write of each of <paths>; with <fresh>, counting
    /// every write made before the question, which costs the time FSEvents
    /// takes to deliver one (about a second under load). Without it the
    /// answer may miss the last second's writes.
    pub fn measure(&self, paths: &[PathBuf], fresh: bool) -> Vec<Answer> {
        if !self.flags.lock().expect("no panic holding it").ready || (fresh && !self.catch_up()) {
            return paths.iter().map(|_| None).collect();
        }
        let flags = self.flags.lock().expect("no panic holding it");
        if !flags.ready {
            return paths.iter().map(|_| None).collect();
        }
        let index = self.index.lock().expect("no panic holding it");
        paths
            .iter()
            .map(|path| {
                if !path.starts_with(&self.root)
                    || self.left_out(path)
                    || flags.rescanning.iter().any(|scan| path.starts_with(scan) || scan.starts_with(path))
                {
                    return None;
                }
                Some(index.lookup(path).map(|id| index.measure(id)).unwrap_or_default())
            })
            .collect()
    }

    /// The number of entries followed and the memory they take, roughly.
    pub fn size(&self) -> (usize, usize) {
        let index = self.index.lock().expect("no panic holding it");
        (index.len(), index.footprint())
    }

    /// Blocks until the whole root is followed.
    #[cfg(test)]
    pub fn wait_ready(&self) {
        self.wait_walks(1);
    }

    /// Blocks until the root has been walked whole <n> times and is followed.
    #[cfg(test)]
    pub fn wait_walks(&self, n: u64) {
        let mut flags = self.flags.lock().expect("no panic holding it");
        while !(flags.ready && flags.walks >= n) {
            flags = self.wake.wait(flags).expect("no panic holding it");
        }
    }
}

#[cfg(test)]
#[path = "tracker_tests.rs"]
mod tests;
