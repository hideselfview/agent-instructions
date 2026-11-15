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
//! - Lost events: FSEvents drops events when its own daemon falls behind, even
//!   for a client that discards every batch (measured under bulk writes), so
//!   losing events is normal. A loss (USER_DROPPED or KERNEL_DROPPED) names
//!   the root, and FSEvents' history still holds the events: the stream is
//!   started again from the last event id handled before the loss and
//!   replays the rest, folder by folder, with no walk. A loss during a replay
//!   starts it again from where it got to; one before it got anywhere, or
//!   event ids wrapping, walks the whole root into a new index. A scan hint
//!   alone (MUST_SCAN_SUBDIRS) names a folder, which is walked again into an
//!   index of its own and grafted in place of what the index held for it.
//!   Walks run on a thread of their own, one at a time, queued without
//!   duplicates, so applying events and answering marks never wait on one;
//!   events applied while a walk runs are applied again to what the walk puts
//!   in place. A mark whose event was lost is delivered again by the replay;
//!   a walk instead counts every mark taken before it as delivered.
//! - The root changing or its volume unmounting stops the stream; a kqueue
//!   watch on the root's parent (vnode.rs: FSEvents delivers too late to say
//!   when the root came back) reports it coming back, and then the stream
//!   starts again and the root is walked. Nothing polls.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::Metadata;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use super::fsevents::{Event, Stream, flags};
use super::index::{Entry, Index, KindOf, Listed, NodeId, ROOT, gone, list_metadata, walk};
use super::vnode::Watch;
use crate::usage::Measure;

const WORKERS: usize = 8;

#[derive(Default)]
struct Flags {
    /// The index holds the whole root.
    ready: bool,
    /// Events that arrived while the index was being walked.
    pending: Vec<Event>,
    /// The root's (dev, inode) as the supervisor found it when it last took
    /// up a restart; None when the root was gone then.
    followed: Option<(u64, u64)>,
    /// The root is not the folder `followed` names (or came back), and the
    /// supervisor has not yet taken that up.
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

/// How far the stream's events have been handled.
#[derive(Default)]
struct Ids {
    /// The highest event id handled.
    last: u64,
    /// While the stream replays what it lost: the id the replay runs to.
    replay_until: Option<u64>,
    /// The id the running replay started after.
    replay_after: u64,
}

/// The folders to walk again because FSEvents lost events, and what the
/// walks must make up for.
#[derive(Default)]
struct Rescans {
    /// Folders waiting for a walk, each with the flags that asked and the
    /// index it was asked for.
    pending: Vec<(PathBuf, u32, u64)>,
    /// A walk is running: every event applied meanwhile is in `replay`.
    running: bool,
    /// The folders of events applied to the current index since the running
    /// walk began. The walk's own index may have read a folder before such an
    /// event changed it, so each is applied again to the index that replaces
    /// the old one.
    replay: Vec<PathBuf>,
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
    /// Which index the tracker holds: the number of times the supervisor has
    /// put a fresh one in place. Whoever reads the disk into the index (an
    /// applied batch, a whole-root rescan) holds this lock from the moment it
    /// takes the index's entry ids to the last write that uses them, so the
    /// supervisor never replaces the index under it, and drops a batch that
    /// was queued for an index since replaced (the walk of the new index
    /// reads what that batch changed).
    epoch: Mutex<u64>,
    flags: Mutex<Flags>,
    wake: Condvar,
    /// Handled event ids; the apply thread alone writes it.
    ids: Mutex<Ids>,
    /// Walks FSEvents' lost events asked for; one thread takes them, so a
    /// walk never holds up applying events or answering a mark.
    rescans: Mutex<Rescans>,
    rescan_wake: Condvar,
    me: std::sync::Weak<Tracker>,
    stream: Mutex<Option<Stream>>,
    /// Events delivered and not yet applied.
    queue: Mutex<Vec<Event>>,
    queued: Condvar,
    /// How many events the root's stream has delivered, for a failing test to
    /// print.
    #[cfg(test)]
    heard: std::sync::atomic::AtomicU64,
    /// Follows the root's parent, to learn the root came back.
    _parent: Option<Watch>,
}

/// One folder's answer: its size and newest write, or None when the daemon
/// can't say (still walking it, or outside the root).
pub type Answer = Option<Measure>;

impl Tracker {
    /// Starts following <root> (an absolute, resolved path).
    pub fn start(root: &Path, marks_dir: &Path) -> std::io::Result<Arc<Tracker>> {
        Self::start_waiting(root, marks_dir, std::time::Duration::from_secs(4))
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
            let weak_self = me.clone();
            let parent = root.parent().and_then(|parent| {
                Watch::start(parent, move || {
                    if let Some(tracker) = weak.upgrade() {
                        tracker.parent_changed();
                    }
                })
                .inspect_err(|error| {
                    eprintln!(
                        "builds: watching {}: {error}; a root that comes back goes unseen",
                        parent.display()
                    );
                })
                .ok()
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
                epoch: Mutex::new(0),
                flags: Mutex::new(Flags {
                    restart: true,
                    ..Flags::default()
                }),
                wake: Condvar::new(),
                ids: Mutex::new(Ids::default()),
                me: weak_self,
                rescans: Mutex::new(Rescans::default()),
                rescan_wake: Condvar::new(),
                stream: Mutex::new(None),
                queue: Mutex::new(Vec::new()),
                queued: Condvar::new(),
                #[cfg(test)]
                heard: std::sync::atomic::AtomicU64::new(0),
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
        let rescanner = tracker.clone();
        std::thread::Builder::new()
            .name("builds-rescan".into())
            .spawn(move || rescanner.rescan_loop())?;
        Ok(tracker)
    }

    /// The root's (dev, inode) now; None when it is gone.
    fn root_identity(&self) -> Option<(u64, u64)> {
        std::fs::symlink_metadata(&self.root)
            .ok()
            .map(|meta| (meta.dev(), meta.ino()))
    }

    /// Asks the supervisor to follow the root again when the folder at its
    /// path is not the one the supervisor took up last (or came back after
    /// it was gone), and says whether it asked. Every signal that reports the
    /// same replacement (the root's stream, the parent's stream, events of
    /// the old stream still queued) finds the supervisor already asked, or
    /// already following the new folder, so one replacement is one restart.
    /// The supervisor reads the root's identity under the same lock when it
    /// takes up a restart, so a signal never sees a half-made change.
    fn signal_restart(&self, why: &str) -> bool {
        let mut flags = self.flags.lock().expect("no panic holding it");
        if flags.restart || self.root_identity() == flags.followed {
            return false;
        }
        eprintln!("builds: {} {why}; following it again", self.root.display());
        flags.restart = true;
        flags.ready = false;
        self.wake.notify_all();
        true
    }

    /// Something changed beside the root: it may have come back, or been
    /// replaced.
    fn parent_changed(&self) {
        self.signal_restart("changed beside it");
    }

    /// Starts the root's stream and walks the root whenever a restart is
    /// due and the root is there.
    fn supervise(self: Arc<Self>) {
        loop {
            let meta = {
                let mut flags = self.flags.lock().expect("no panic holding it");
                while !flags.restart {
                    flags = self.wake.wait(flags).expect("no panic holding it");
                }
                flags.restart = false;
                flags.ready = false;
                flags.pending.clear();
                // Stops the old stream, and reads the root, with the flags
                // held: a signal then sees the old folder followed and the
                // restart pending, or this folder followed, never in between.
                *self.stream.lock().expect("no panic holding it") = None;
                let meta = std::fs::symlink_metadata(&self.root);
                flags.followed = meta.as_ref().ok().map(|meta| (meta.dev(), meta.ino()));
                meta
            };
            let Ok(meta) = meta else {
                eprintln!(
                    "builds: {} is gone; waiting for it to come back",
                    self.root.display()
                );
                continue;
            };
            let epoch = {
                let mut epoch = self.epoch.lock().expect("no panic holding it");
                *epoch += 1;
                *self.index.lock().expect("no panic holding it") = Index::new(&self.root, &meta);
                *epoch
            };
            let since = Stream::current_event_id();
            *self.ids.lock().expect("no panic holding it") = Ids {
                last: since,
                replay_until: None,
                replay_after: 0,
            };
            let stream = self.open_stream(since);
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
                self.apply(pending, epoch);
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

    /// Starts the root's stream, replaying every event after <since>.
    fn open_stream(&self, since: u64) -> Option<Stream> {
        let me = self.me.clone();
        let trash = self.root.join(".trash");
        Stream::start_since(
            since,
            &[self.root.as_path()],
            &[trash.as_path()],
            0.2,
            Box::new(move |events| {
                if let Some(tracker) = me.upgrade() {
                    #[cfg(test)]
                    tracker
                        .heard
                        .fetch_add(events.len() as u64, std::sync::atomic::Ordering::SeqCst);
                    tracker
                        .queue
                        .lock()
                        .expect("no panic holding it")
                        .extend(events);
                    tracker.queued.notify_all();
                }
            }),
        )
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
        let epoch = *self.epoch.lock().expect("no panic holding it");
        self.handle(events, epoch);
        self.tell(&paths);
        if mark > 0 {
            let mut marks = self.marks.lock().expect("no panic holding it");
            marks.seen = marks.seen.max(mark);
            self.delivered.notify_all();
        }
    }

    fn handle(&self, events: Vec<Event>, epoch: u64) {
        // The stream also says ROOT_CHANGED when a folder above the root
        // changes, the root itself staying where it was.
        let root_changed = events.iter().any(|event| {
            event.flags & (flags::ROOT_CHANGED | flags::UNMOUNT) != 0
                && event.path.starts_with(&self.root)
        });
        if root_changed && self.signal_restart("moved or unmounted") {
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
        // FSEvents drops events when its daemon falls behind, whatever the
        // client does, and reports it as USER_DROPPED or KERNEL_DROPPED on an
        // event that names the root. Its history still holds them: the
        // stream is started again from the last id handled before the loss,
        // and replays what came after, folder by folder. A loss during that
        // replay starts it again from where it got to; one before it got
        // anywhere, or event ids wrapping (the ids no longer order events),
        // leaves only a walk of the root. A scan hint without a loss
        // (MUST_SCAN_SUBDIRS alone) names the one folder to walk again.
        let mut lost_after: Option<u64> = None;
        let mut walk_root: Option<u32> = None;
        let mut scans: Vec<(PathBuf, u32)> = Vec::new();
        let mut others: Vec<Event> = Vec::new();
        {
            let mut ids = self.ids.lock().expect("no panic holding it");
            for event in events {
                let lost = event.flags & (flags::USER_DROPPED | flags::KERNEL_DROPPED) != 0;
                if event.flags & flags::EVENT_IDS_WRAPPED != 0 {
                    walk_root = Some(event.flags);
                } else if lost {
                    // A replay that lost events before handling any new one
                    // makes no progress; a walk of the root ends the loop.
                    if ids.replay_until.is_some() && ids.last <= ids.replay_after {
                        walk_root = Some(event.flags);
                    } else {
                        lost_after.get_or_insert(ids.last);
                    }
                } else {
                    if event.flags & flags::MUST_SCAN_SUBDIRS != 0 {
                        if event.path.starts_with(&self.root) && !self.left_out(&event.path) {
                            scans.push((event.path, event.flags));
                        }
                    } else {
                        others.push(event.clone());
                    }
                    ids.last = ids.last.max(event.id);
                    if ids.replay_until.is_some_and(|until| event.id >= until) {
                        ids.replay_until = None;
                    }
                }
            }
        }
        if walk_root.is_some() {
            lost_after = None;
            // The loss may include a mark's event, which a walk does not
            // replay: every mark taken so far counts as delivered.
            let mut marks = self.marks.lock().expect("no panic holding it");
            marks.seen = marks.seen.max(marks.taken);
            self.delivered.notify_all();
            scans.clear();
            scans.push((self.root.clone(), walk_root.unwrap_or(0)));
            self.ids.lock().expect("no panic holding it").replay_until = None;
        }
        // The events that arrived with the lost ones are applied now, so a
        // question's mark is answered from them.
        self.record_for_replay(&others);
        self.apply(others, epoch);
        if let Some(after) = lost_after {
            self.replay_from(after, epoch);
        }
        self.request_rescans(scans, epoch);
    }

    /// Starts the stream again from <after>, the last id handled before
    /// events were lost.
    fn replay_from(&self, after: u64, epoch: u64) {
        let held = self.epoch.lock().expect("no panic holding it");
        if *held != epoch {
            return;
        }
        let until = Stream::current_event_id();
        {
            let mut ids = self.ids.lock().expect("no panic holding it");
            ids.replay_until = Some(until);
            ids.replay_after = after;
        }
        eprintln!(
            "{} builds: FSEvents lost events; replaying its history from event {after} to {until}",
            crate::text::clock()
        );
        let Some(stream) = self.open_stream(after) else {
            eprintln!("builds: FSEvents would not replay; walking the root");
            self.ids.lock().expect("no panic holding it").replay_until = None;
            drop(held);
            self.request_rescans(vec![(self.root.clone(), 0)], epoch);
            return;
        };
        // The stream the loss came on stops; whatever it still holds queued
        // is handled, and the replay repeats it harmlessly.
        *self.stream.lock().expect("no panic holding it") = Some(stream);
    }

    /// Notes the folders of <events>, about to be applied, for the running
    /// walk to apply again.
    fn record_for_replay(&self, events: &[Event]) {
        let mut rescans = self.rescans.lock().expect("no panic holding it");
        if rescans.running {
            rescans
                .replay
                .extend(events.iter().map(|event| event.path.clone()));
        }
    }

    /// Queues walks of <folders>; a folder under another one queued, or any
    /// folder when the root is queued, is walked with it.
    fn request_rescans(&self, folders: Vec<(PathBuf, u32)>, epoch: u64) {
        if folders.is_empty() {
            return;
        }
        let mut rescans = self.rescans.lock().expect("no panic holding it");
        for (folder, flags) in folders {
            if rescans
                .pending
                .iter()
                .any(|(queued, _, _)| folder.starts_with(queued))
            {
                continue;
            }
            rescans
                .pending
                .retain(|(queued, _, _)| !queued.starts_with(&folder));
            rescans.pending.push((folder, flags, epoch));
        }
        self.rescan_wake.notify_all();
    }

    /// Walks the queued folders again, one at a time, for as long as the
    /// daemon runs.
    fn rescan_loop(&self) {
        loop {
            let (folder, flags, epoch, from) = {
                let mut rescans = self.rescans.lock().expect("no panic holding it");
                while rescans.pending.is_empty() {
                    rescans.running = false;
                    rescans.replay.clear();
                    self.rescan_wake.notify_all();
                    rescans = self.rescan_wake.wait(rescans).expect("no panic holding it");
                }
                let (folder, flags, epoch) = rescans.pending.remove(0);
                rescans.running = true;
                let from = rescans.replay.len();
                (folder, flags, epoch, from)
            };
            self.rewalk(&folder, flags, epoch, from);
        }
    }

    /// Walks <folder> (the root, or a folder below it) again into an index
    /// of its own and puts that in place of what the index held for it: the
    /// current index answers questions until then. Events applied meanwhile
    /// (`replay` from <replay_from>) are applied again to what replaces it.
    fn rewalk(&self, folder: &Path, flags: u32, epoch: u64, replay_from: usize) {
        let started = std::time::Instant::now();
        eprintln!(
            "{} builds: FSEvents lost events (flags {flags:#x}); walking {} again",
            crate::text::clock(),
            folder.display()
        );
        let meta = match std::fs::symlink_metadata(folder) {
            Ok(meta) if meta.is_dir() => meta,
            _ if *folder == self.root => {
                self.signal_restart("is gone");
                return;
            }
            // Gone, or no longer a folder: what is there now is read like
            // any change.
            _ => {
                self.apply(
                    vec![Event {
                        path: folder.to_path_buf(),
                        flags: 0,
                        id: 0,
                    }],
                    epoch,
                );
                return;
            }
        };
        let replay: Vec<PathBuf>;
        let entries = if *folder == self.root {
            let fresh = Mutex::new(Index::new(&self.root, &meta));
            walk(&fresh, 0, &self.root, WORKERS);
            let fresh = fresh.into_inner().expect("no panic holding it");
            let entries = fresh.len();
            // A restart since this walk began put a newer index in place.
            let held = self.epoch.lock().expect("no panic holding it");
            if *held != epoch {
                return;
            }
            *self.index.lock().expect("no panic holding it") = fresh;
            let mut state = self.flags.lock().expect("no panic holding it");
            state.walks += 1;
            self.wake.notify_all();
            drop(state);
            replay = self.replay_since(replay_from);
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
                self.apply(
                    vec![Event {
                        path: folder.to_path_buf(),
                        flags: 0,
                        id: 0,
                    }],
                    epoch,
                );
                return;
            }
            let fresh = Mutex::new(Index::of_folder(folder, &meta));
            walk(&fresh, 0, folder, WORKERS);
            let fresh = fresh.into_inner().expect("no panic holding it");
            let held = self.epoch.lock().expect("no panic holding it");
            if *held != epoch {
                return;
            }
            let mut index = self.index.lock().expect("no panic holding it");
            match index.lookup(folder) {
                Some(id) if index.is_dir(id) => index.graft(id, &fresh),
                // Gone from the index meanwhile: the events that took it
                // are applied next.
                _ => {}
            }
            replay = self.replay_since(replay_from);
            fresh.len()
        };
        self.apply(
            replay
                .into_iter()
                .map(|path| Event {
                    path,
                    flags: 0,
                    id: 0,
                })
                .collect(),
            epoch,
        );
        eprintln!(
            "{} builds: walked {} again: {entries} entries in {:.1}s",
            crate::text::clock(),
            folder.display(),
            started.elapsed().as_secs_f64()
        );
    }

    /// The folders of events applied since the walk that began at <from>.
    fn replay_since(&self, from: usize) -> Vec<PathBuf> {
        let rescans = self.rescans.lock().expect("no panic holding it");
        rescans.replay.get(from..).unwrap_or_default().to_vec()
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
    fn apply(&self, events: Vec<Event>, epoch: u64) {
        let held = self.epoch.lock().expect("no panic holding it");
        if *held != epoch {
            return;
        }
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
                        "{} builds: FSEvents took {:.1}s to deliver {}",
                        crate::text::clock(),
                        took.as_secs_f64(),
                        mark.display()
                    );
                }
                break true;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                eprintln!(
                    "{} builds: FSEvents did not deliver {} in {}s; answering nothing",
                    crate::text::clock(),
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

    /// What decides whether a question gets an answer, for a failing test to
    /// print.
    #[cfg(test)]
    pub fn describe(&self) -> String {
        let flags = self.flags.lock().expect("no panic holding it");
        let marks = self.marks.lock().expect("no panic holding it");
        let now = std::fs::symlink_metadata(&self.root)
            .ok()
            .map(|meta| (meta.dev(), meta.ino()));
        format!(
            "ready {}, restart {}, followed {:?}, root now {now:?}, pending {}, walks {}, \
             marks taken {} seen {}, queued {}, events heard {}",
            flags.ready,
            flags.restart,
            flags.followed,
            flags.pending.len(),
            flags.walks,
            marks.taken,
            marks.seen,
            self.queue.lock().expect("no panic holding it").len(),
            self.heard.load(std::sync::atomic::Ordering::SeqCst),
        )
    }

    /// Blocks until every walk FSEvents' lost events asked for has ended.
    #[cfg(test)]
    pub fn wait_rescans(&self) {
        let mut rescans = self.rescans.lock().expect("no panic holding it");
        while rescans.running || !rescans.pending.is_empty() {
            rescans = self.rescan_wake.wait(rescans).expect("no panic holding it");
        }
    }

    /// Blocks until the whole root is followed.
    #[cfg(test)]
    pub fn wait_ready(&self) {
        self.wait_walks(1);
    }

    /// Blocks until the whole root is walked and followed as the folder now
    /// at the root's path. A count of walks does not say the last restart is
    /// done (a root replaced twice is walked twice); the folder's identity
    /// does.
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
            if left.is_zero() {
                // Release the lock first: a panic holding it would poison it
                // for every other thread and bury this failure under theirs.
                drop(flags);
                panic!(
                    "the root was never followed again; tracker: {}",
                    self.describe()
                );
            }
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
