//! Keeps the index of the build volume current: a full walk when the daemon
//! starts or the volume comes back, then every change FSEvents reports.
//!
//! - The stream on the root starts before the walk, from an event id taken
//!   before it (a stream is not live the moment it starts, and replays what
//!   came after that id); events that arrive during the walk wait and are
//!   applied after it.
//! - The stream's callback only queues its events; one thread applies them,
//!   everything queued at once, so delivery never waits on a walk.
//! - The stream reports folders, not files: one event per folder something
//!   changed in, however many of its files a build wrote, so FSEvents carries
//!   a few events a second through a build instead of one per file written.
//! - Applying an event never trusts the event's flags for what a folder holds
//!   now: it lists the folder. Gone: its entry and everything under it go.
//!   Entries the listing no longer has go. A listed entry whose inode and
//!   change time are those the index records is left as it is; any other is
//!   read again. A folder that is new or has a new inode is walked, or a
//!   folder gone from a listing in the same batch with that inode is put back
//!   under its new name (a rename, such as an eviction into .trash, moves
//!   100k entries without a walk).
//! - The root's macOS entries and its trash are not followed (see
//!   `places::unfollowed`, which also names the trash's lock and the
//!   `.trash-emptying.*` folders it is renamed to); a question about them gets no answer.
//! - Lost events: MUST_SCAN_SUBDIRS names the folder under which FSEvents
//!   lost them (with USER_DROPPED or KERNEL_DROPPED when it dropped a batch),
//!   and that folder is walked again into an index of its own, grafted in
//!   place of what the index held for it. A dropped batch with no folder
//!   named, or event ids wrapping, walks the whole root the same way into a
//!   new index. The current index answers meanwhile. A deciding command's
//!   question waits for its mark, which is applied after the walk, so its
//!   answer counts every write; `status` and the page get the old sizes
//!   meanwhile.
//! - The root changing or its volume unmounting stops the stream; a stream
//!   on the root's parent reports it coming back, and then the stream starts
//!   again and the root is walked. Nothing polls.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::Metadata;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use super::fsevents::{Event, Stream, flags};
use super::index::{Entry, Index, KindOf, Listed, NodeId, ROOT, gone, list_metadata, walk};
use crate::usage::Measure;

const WORKERS: usize = 8;

#[derive(Default)]
struct Flags {
    /// The index holds the whole root.
    ready: bool,
    /// Events that arrived while the index was being walked.
    pending: Vec<Event>,
    /// The root's (dev, inode) when its stream started.
    followed: Option<(u64, u64)>,
    /// The root's stream needs starting again.
    restart: bool,
    /// How many times the root has been walked whole.
    walks: u64,
}

/// The marks questions write, and how far the stream has delivered them. A
/// mark is a folder with a file in it: writing the file is an event for the
/// mark's own folder, which names the mark.
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
    /// This daemon's marks are `<marks_prefix><n>`: a name no other daemon
    /// used, so removing a killed daemon's marks at start, whose events
    /// arrive after, never reads as delivery of one of this daemon's.
    marks_prefix: String,
    /// Where questions write their marks: a folder `<marks_prefix><n>` whose
    /// event, once handled, says every write made before the question has
    /// been.
    marks_dir: PathBuf,
    marks: Mutex<Marks>,
    delivered: Condvar,
    /// How long a question waits for its mark's event before answering
    /// nothing.
    mark_wait: std::time::Duration,
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
        Self::start_waiting(root, marks_dir, std::time::Duration::from_secs(10))
    }

    /// Starts following <root>; a question waits up to <mark_wait> for the
    /// stream to deliver its mark. How long FSEvents takes depends on every
    /// write on the machine, so a caller that must have the answer gives a
    /// wait it will not reach unless the stream is stuck.
    pub fn start_waiting(
        root: &Path,
        marks_dir: &Path,
        mark_wait: std::time::Duration,
    ) -> std::io::Result<Arc<Tracker>> {
        let meta = std::fs::symlink_metadata(root)?;
        std::fs::create_dir_all(marks_dir)?;
        // Marks a daemon killed mid-question left; no question waits on
        // them, since one daemon follows a root.
        for entry in std::fs::read_dir(marks_dir)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("mark-"))
            {
                if entry.file_type()?.is_dir() {
                    std::fs::remove_dir_all(entry.path())?;
                } else {
                    std::fs::remove_file(entry.path())?;
                }
            }
        }
        let tracker = Arc::new_cyclic(|me: &std::sync::Weak<Tracker>| {
            let weak = me.clone();
            let parent = root.parent().and_then(|parent| {
                Stream::start(
                    &[parent],
                    &[],
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
                marks_prefix: format!(
                    "mark-{}-{}-",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                ),
                marks: Mutex::new(Marks::default()),
                delivered: Condvar::new(),
                mark_wait,
                index: Mutex::new(Index::new(root, &meta)),
                flags: Mutex::new(Flags {
                    restart: true,
                    ..Flags::default()
                }),
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
                eprintln!(
                    "builds: {} is gone; waiting for it to come back",
                    self.root.display()
                );
                continue;
            };
            *self.index.lock().expect("no panic holding it") = Index::new(&self.root, &meta);
            self.flags.lock().expect("no panic holding it").followed =
                Some((meta.dev(), meta.ino()));
            let me = Arc::downgrade(&self);
            let trash = self.root.join(".trash");
            let stream = Stream::start(
                &[self.root.as_path()],
                &[trash.as_path()],
                0.2,
                Box::new(move |events| {
                    if let Some(tracker) = me.upgrade() {
                        tracker
                            .queue
                            .lock()
                            .expect("no panic holding it")
                            .extend(events);
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
        let now = std::fs::symlink_metadata(&self.root)
            .ok()
            .map(|meta| (meta.dev(), meta.ino()));
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
            .filter_map(|event| {
                event
                    .path
                    .file_name()?
                    .to_str()?
                    .strip_prefix(&self.marks_prefix)?
                    .parse::<u64>()
                    .ok()
            })
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
            event.flags & (flags::ROOT_CHANGED | flags::UNMOUNT) != 0
                && event.path.starts_with(&self.root)
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
        // FSEvents names the folder under which it lost events with
        // MUST_SCAN_SUBDIRS; that folder alone is walked again. A lost batch
        // with no folder named, or event ids wrapping, says nothing narrower
        // than the root.
        let whole = events.iter().find(|event| {
            let lost = event.flags & (flags::USER_DROPPED | flags::KERNEL_DROPPED) != 0;
            (lost && event.flags & flags::MUST_SCAN_SUBDIRS == 0)
                || event.flags & flags::EVENT_IDS_WRAPPED != 0
        });
        if let Some(event) = whole {
            // Every other event in the batch came before the walk, which
            // reads what they changed.
            let flags = event.flags;
            self.rewalk(&self.root.clone(), flags);
            return;
        }
        let (scans, others): (Vec<Event>, Vec<Event>) = events
            .into_iter()
            .partition(|event| event.flags & flags::MUST_SCAN_SUBDIRS != 0);
        self.apply(others);
        // A folder under another one walked again is walked with it.
        let mut folders: Vec<(PathBuf, u32)> = scans
            .into_iter()
            .filter(|event| event.path.starts_with(&self.root) && !self.left_out(&event.path))
            .map(|event| (event.path, event.flags))
            .collect();
        folders.sort();
        let mut walked: Vec<PathBuf> = Vec::new();
        for (folder, flags) in folders {
            if walked.iter().any(|done| folder.starts_with(done)) {
                continue;
            }
            self.rewalk(&folder, flags);
            walked.push(folder);
        }
    }

    /// Walks <folder> (the root, or a folder below it) again into an index
    /// of its own and puts that in place of what the index held for it: the
    /// current index answers questions until then. Events that arrive
    /// meanwhile wait in the queue and are applied after.
    fn rewalk(&self, folder: &Path, flags: u32) {
        let started = std::time::Instant::now();
        let meta = match std::fs::symlink_metadata(folder) {
            Ok(meta) if meta.is_dir() => meta,
            _ if *folder == self.root => {
                self.signal_restart("is gone");
                return;
            }
            // Gone, or no longer a folder: what is there now is read like
            // any change.
            _ => {
                self.apply(vec![Event {
                    path: folder.to_path_buf(),
                    flags: 0,
                }]);
                return;
            }
        };
        let entries = if *folder == self.root {
            let fresh = Mutex::new(Index::new(&self.root, &meta));
            walk(&fresh, 0, &self.root, WORKERS);
            let fresh = fresh.into_inner().expect("no panic holding it");
            let entries = fresh.len();
            *self.index.lock().expect("no panic holding it") = fresh;
            let mut state = self.flags.lock().expect("no panic holding it");
            state.walks += 1;
            self.wake.notify_all();
            entries
        } else {
            if self
                .index
                .lock()
                .expect("no panic holding it")
                .lookup(folder)
                .is_none()
            {
                // A folder the index doesn't hold yet: applying it walks it.
                self.apply(vec![Event {
                    path: folder.to_path_buf(),
                    flags: 0,
                }]);
                return;
            }
            let fresh = Mutex::new(Index::of_folder(folder, &meta));
            walk(&fresh, 0, folder, WORKERS);
            let fresh = fresh.into_inner().expect("no panic holding it");
            let mut index = self.index.lock().expect("no panic holding it");
            match index.lookup(folder) {
                Some(id) if index.is_dir(id) => index.graft(id, &fresh),
                // Gone from the index meanwhile: the events that took it
                // are applied next.
                _ => {}
            }
            fresh.len()
        };
        eprintln!(
            "builds: FSEvents lost events (flags {flags:#x}); walked {} again: {entries} entries in {:.1}s",
            folder.display(),
            started.elapsed().as_secs_f64()
        );
    }

    /// Is <path> in one of the root's entries the index leaves out?
    fn left_out(&self, path: &Path) -> bool {
        let Ok(rest) = path.strip_prefix(&self.root) else {
            return false;
        };
        let first = rest
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned());
        first.is_some_and(|name| crate::places::unfollowed(&name))
    }

    /// Lists each changed folder and records what it holds now.
    fn apply(&self, events: Vec<Event>) {
        // Each folder once, parents before children.
        let folders: BTreeSet<PathBuf> = events
            .into_iter()
            .map(|event| event.path)
            .filter(|path| path.starts_with(&self.root) && !self.left_out(path))
            .collect();
        let mut read: Vec<Listing> = Vec::new();
        for path in folders {
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if gone(&error) => {
                    read.push(Listing {
                        path,
                        meta: None,
                        names: None,
                        listed: Vec::new(),
                    });
                    continue;
                }
                Err(error) => {
                    eprintln!(
                        "builds: reading {}: {error}; left as it was",
                        path.display()
                    );
                    continue;
                }
            };
            if !meta.is_dir() {
                read.push(Listing {
                    path,
                    meta: Some(meta),
                    names: None,
                    listed: Vec::new(),
                });
                continue;
            }
            match list_metadata(&path) {
                Ok(Some(mut listed)) => {
                    if path == self.root {
                        listed.retain(|one| {
                            !std::str::from_utf8(&one.name).is_ok_and(crate::places::unfollowed)
                        });
                    }
                    let names = Some(listed.iter().map(|one| one.name.clone()).collect());
                    read.push(Listing {
                        path,
                        meta: Some(meta),
                        names,
                        listed,
                    });
                }
                Ok(None) => read.push(Listing {
                    path,
                    meta: None,
                    names: None,
                    listed: Vec::new(),
                }),
                Err(error) => eprintln!(
                    "builds: listing {}: {error}; left as it was",
                    path.display()
                ),
            }
        }
        // Only the entries that changed are read further (a big file's
        // extents), and outside the lock.
        {
            let index = self.index.lock().expect("no panic holding it");
            for listing in &mut read {
                let Some(meta) = &listing.meta else { continue };
                let held = index
                    .lookup(&listing.path)
                    .filter(|id| index.is_dir(*id) && index.ino(*id) == meta.ino());
                if let Some(id) = held {
                    listing
                        .listed
                        .retain(|one| !index.unchanged(id, &one.name, &one.meta));
                }
            }
        }
        let changed: Vec<Changed> = read
            .into_iter()
            .map(
                |Listing {
                     path,
                     meta,
                     names,
                     listed,
                 }| {
                    let own = meta.map(|meta| {
                        let name = path
                            .file_name()
                            .map(|n| n.as_encoded_bytes().to_vec())
                            .unwrap_or_default();
                        Entry::read(&name, &path, &meta)
                    });
                    let entries = listed
                        .iter()
                        .map(|one| Entry::read(&one.name, &one.path, &one.meta))
                        .collect();
                    Changed {
                        path,
                        own,
                        names,
                        entries,
                    }
                },
            )
            .collect();
        let mut walks: Vec<(NodeId, PathBuf)> = Vec::new();
        {
            let mut index = self.index.lock().expect("no panic holding it");
            // Gone first, so a folder renamed within the batch is found again
            // by its inode wherever the batch lists it.
            let mut limbo: HashMap<u64, NodeId> = HashMap::new();
            for one in &changed {
                let Some(id) = index.lookup(&one.path) else {
                    continue;
                };
                match (&one.own, &one.names) {
                    (None, _) if one.path != self.root => take(&mut index, id, &mut limbo),
                    (Some(_), Some(names)) if index.is_dir(id) => {
                        for child in unlisted(&index, id, names) {
                            take(&mut index, child, &mut limbo);
                        }
                    }
                    _ => {}
                }
            }
            for one in changed {
                let Some(own) = one.own else { continue };
                if walks.iter().any(|(_, walked)| one.path.starts_with(walked)) {
                    continue;
                }
                let id = if one.path == self.root {
                    index.set_root(&own);
                    ROOT
                } else {
                    let parent_path = one
                        .path
                        .parent()
                        .expect("a path below the root has a parent");
                    let Some(parent) = index.lookup(parent_path) else {
                        // A folder the index lost track of: walk from the
                        // first one missing.
                        let mut missing = one.path.as_path();
                        while let Some(up) = missing.parent() {
                            if index.lookup(up).is_some() {
                                break;
                            }
                            missing = up;
                        }
                        if let Ok(meta) = std::fs::symlink_metadata(missing) {
                            let up = index
                                .lookup(missing.parent().expect("below the root"))
                                .expect("found above");
                            let name = missing
                                .file_name()
                                .map(|n| n.as_encoded_bytes().to_vec())
                                .unwrap_or_default();
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
                    let (id, new) = place(&mut index, parent, own, &mut limbo);
                    if new {
                        walks.push((id, one.path.clone()));
                        continue;
                    }
                    id
                };
                let Some(names) = one.names else { continue };
                // A folder put back from limbo still holds what it held.
                for child in unlisted(&index, id, &names) {
                    index.remove(child);
                }
                for entry in one.entries {
                    let path = one.path.join(std::ffi::OsStr::from_bytes(&entry.name));
                    let (child, new) = place(&mut index, id, entry, &mut limbo);
                    if new {
                        walks.push((child, path));
                    }
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
        let mark = self.marks_dir.join(format!("{}{n}", self.marks_prefix));
        if let Err(error) = std::fs::create_dir_all(&self.marks_dir)
            .and_then(|()| std::fs::create_dir(&mark))
            .and_then(|()| std::fs::write(mark.join("mark"), b""))
        {
            eprintln!(
                "builds: writing {}: {error}; answering nothing",
                mark.display()
            );
            return false;
        }
        if let Some(stream) = self.stream.lock().expect("no panic holding it").as_ref() {
            stream.flush();
        }
        let asked = std::time::Instant::now();
        let deadline = asked + self.mark_wait;
        let mut marks = self.marks.lock().expect("no panic holding it");
        let caught_up = loop {
            if marks.seen >= n {
                let took = asked.elapsed();
                if took > std::time::Duration::from_secs(1) {
                    eprintln!(
                        "builds: FSEvents took {:.1}s to deliver {}",
                        took.as_secs_f64(),
                        mark.display()
                    );
                }
                break true;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                eprintln!(
                    "builds: FSEvents did not deliver {} in {}s; answering nothing",
                    mark.display(),
                    self.mark_wait.as_secs()
                );
                break false;
            }
            marks = self
                .delivered
                .wait_timeout(marks, left)
                .expect("no panic holding it")
                .0;
        };
        drop(marks);
        if let Err(error) = std::fs::remove_dir_all(&mark) {
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
                if !path.starts_with(&self.root) || self.left_out(path) {
                    return None;
                }
                Some(
                    index
                        .lookup(path)
                        .map(|id| index.measure(id))
                        .unwrap_or_default(),
                )
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

    /// Blocks until the whole root is walked and followed as the folder now
    /// at the root's path. A replaced root restarts the tracker once for each
    /// signal that reports it (the root's stream, the parent's stream, events
    /// of the old stream still queued), so a count of walks does not say the
    /// last restart is done; the folder's identity does.
    #[cfg(test)]
    pub fn wait_following_current_root(&self) {
        let hang_guard = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut flags = self.flags.lock().expect("no panic holding it");
        loop {
            let now = std::fs::symlink_metadata(&self.root)
                .ok()
                .map(|meta| (meta.dev(), meta.ino()));
            if flags.ready && now.is_some() && flags.followed == now {
                return;
            }
            let left = hang_guard.saturating_duration_since(std::time::Instant::now());
            assert!(!left.is_zero(), "the root was never followed again");
            flags = self
                .wake
                .wait_timeout(flags, left)
                .expect("no panic holding it")
                .0;
        }
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

/// One changed folder as listed, before the index is locked.
struct Listing {
    path: PathBuf,
    /// None when it is gone.
    meta: Option<Metadata>,
    /// The name of every entry it holds, when it is still a folder.
    names: Option<HashSet<Box<[u8]>>>,
    /// Its entries; once compared with the index, those that differ.
    listed: Vec<Listed>,
}

/// One changed folder as it is now.
struct Changed {
    path: PathBuf,
    /// The folder's own facts; None when it is gone.
    own: Option<Entry>,
    /// The name of every entry it holds, when it is still a folder.
    names: Option<HashSet<Box<[u8]>>>,
    /// Its entries that differ from what the index records.
    entries: Vec<Entry>,
}

/// The children of <id> that <names>, its listing, no longer has.
fn unlisted(index: &Index, id: NodeId, names: &HashSet<Box<[u8]>>) -> Vec<NodeId> {
    index
        .children(id)
        .iter()
        .copied()
        .filter(|child| !names.contains(index.name(*child)))
        .collect()
}

/// Takes <id> out of the index: a folder into <limbo> by its inode, to be
/// put back where a listing names it again, anything else for good.
fn take(index: &mut Index, id: NodeId, limbo: &mut HashMap<u64, NodeId>) {
    if index.is_dir(id) {
        index.detach(id);
        limbo.insert(index.ino(id), id);
    } else {
        index.remove(id);
    }
}

/// Records <entry> as <parent>'s child: a folder the index holds keeps what
/// it holds, one in <limbo> with its inode is put back here. Returns the
/// child, and whether it is a folder the index never held, to walk.
fn place(
    index: &mut Index,
    parent: NodeId,
    entry: Entry,
    limbo: &mut HashMap<u64, NodeId>,
) -> (NodeId, bool) {
    if entry.kind != KindOf::Dir {
        return (index.set(parent, entry), false);
    }
    let held = index
        .child_of(parent, &entry.name)
        .is_some_and(|id| index.is_dir(id) && index.ino(id) == entry.ino);
    if held {
        return (index.set(parent, entry), false);
    }
    if let Some(moved) = limbo.remove(&entry.ino) {
        let name = entry.name.clone();
        index.attach(moved, parent, &name);
        return (index.set(parent, entry), false);
    }
    (index.set(parent, entry), true)
}

#[cfg(test)]
#[path = "tracker_tests.rs"]
mod tests;
