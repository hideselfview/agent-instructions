//! macOS FSEvents: a stream of the folders something changed in, under some
//! folders, delivered on a dispatch queue of its own. The system frameworks
//! are declared here by hand; nothing else in the tool needs them.
//!
//! A `Stream` keeps itself following. fseventsd refuses a stream while it
//! starts and after it was killed (it answers every client with an error,
//! and `FSEventsGetCurrentEventId` returns 0), and past its limits of 512
//! streams for one process and 1024 for the system (it names the limit in
//! the unified log). The stream is then opened again with a growing wait,
//! and each refusal's line gives fseventsd's reason. When fseventsd is found
//! restarted (its event id is 0 or went backwards) or the owner says the
//! stream went quiet (`restart`), the stream is made again. Event ids from
//! before a restart mean nothing, so every opening after the first one hands
//! the handler an EVENT_IDS_WRAPPED event for each followed folder, which
//! owners take as "read this folder again". Each step is written to stderr.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

#[cfg(target_os = "macos")]
#[path = "fsevents_macos.rs"]
mod system;
#[cfg(target_os = "linux")]
#[path = "fsevents_linux.rs"]
mod system;

use system::Raw;

/// What an event says about its path.
pub mod flags {
    pub const MUST_SCAN_SUBDIRS: u32 = 0x1;
    pub const USER_DROPPED: u32 = 0x2;
    pub const KERNEL_DROPPED: u32 = 0x4;
    pub const EVENT_IDS_WRAPPED: u32 = 0x8;
    /// Marks the end of the replayed history; its path is not a change.
    pub const HISTORY_DONE: u32 = 0x10;
    /// The item was cloned: a copy on APFS read it, which changed nothing
    /// in it.
    pub const ITEM_CLONED: u32 = 0x40_0000;
    /// What kind of item the event names (a file, a folder, a link, a hard
    /// link, its last hard link), which says nothing about what happened.
    pub const ITEM_KINDS: u32 = 0x1_0000 | 0x2_0000 | 0x4_0000 | 0x10_0000 | 0x20_0000;
}

/// One changed path and what the stream says about it.
#[derive(Clone, Debug)]
pub struct Event {
    pub path: PathBuf,
    pub flags: u32,
}

pub type Handler = Box<dyn Fn(Vec<Event>) + Send + Sync>;

/// How long the supervisor waits between looks at fseventsd.
const WATCH: std::time::Duration = std::time::Duration::from_secs(10);
/// The longest wait between attempts to open a refused stream.
const BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Default)]
struct Wanted {
    stop: bool,
}

struct Shared {
    current: Mutex<Option<Raw>>,
    wanted: Mutex<Wanted>,
    wake: Condvar,
}

/// A stream that keeps itself following (see the module note); dropping it
/// stops it.
pub struct Stream {
    shared: Arc<Shared>,
}

/// Does fseventsd keep an event store for the device <device> (a `st_dev`)?
pub fn device_keeps_events(device: u64) -> bool {
    system::device_keeps_events(device)
}

impl Stream {
    /// The id of the latest event the system has recorded; 0 when fseventsd
    /// does not answer. A stream started from it reports every change made
    /// after it was taken.
    pub fn current_event_id() -> u64 {
        system::current_event_id()
    }

    /// Starts following <folders> as `start_since` does, from the event id
    /// current now.
    pub fn start(folders: &[&Path], excluded: &[&Path], latency: f64, handler: Handler) -> Stream {
        Self::start_since(
            Self::current_event_id(),
            folders,
            excluded,
            latency,
            handler,
        )
    }

    /// Starts following <folders>, leaving out what happens under <excluded>
    /// (at most 8), calling <handler> with each batch of events on a queue of
    /// the stream's own, in order, one batch at a time. The stream replays
    /// every event after <since> (an id from `current_event_id`), so a change
    /// made after it was taken and before the stream went live is reported,
    /// not lost; the replay's end marker is not handed on. With `WatchRoot`,
    /// a change to a followed folder's own path arrives as ROOT_CHANGED.
    /// When the system refuses the stream, it is opened again as the module
    /// note says; until it opens, nothing is reported.
    pub fn start_since(
        since: u64,
        folders: &[&Path],
        excluded: &[&Path],
        latency: f64,
        handler: Handler,
    ) -> Stream {
        let folders: Vec<PathBuf> = folders.iter().map(|p| p.to_path_buf()).collect();
        let excluded: Vec<PathBuf> = excluded.iter().map(|p| p.to_path_buf()).collect();
        let handler: Arc<dyn Fn(Vec<Event>) + Send + Sync> = Arc::from(handler);
        let shared = Arc::new(Shared {
            current: Mutex::new(None),
            wanted: Mutex::new(Wanted::default()),
            wake: Condvar::new(),
        });
        let open = {
            let (folders, excluded, handler) = (folders.clone(), excluded.clone(), handler.clone());
            move |since: u64| {
                let folder_refs: Vec<&Path> = folders.iter().map(PathBuf::as_path).collect();
                let excluded_refs: Vec<&Path> = excluded.iter().map(PathBuf::as_path).collect();
                let handler = handler.clone();
                Raw::open(
                    since,
                    &folder_refs,
                    &excluded_refs,
                    latency,
                    Box::new(move |events| handler(events)),
                )
            }
        };
        // The first try is made here, so a stream that opens is following
        // before this returns.
        let first = open(since);
        let mut opened = false;
        match first {
            Ok(raw) => {
                *shared.current.lock().expect("no panic holding it") = Some(raw);
                opened = true;
            }
            Err(reason) => crate::output::warn(format!(
                "{} builds: {reason}; trying again",
                crate::text::clock()
            )),
        }
        let supervisor = shared.clone();
        let wrapped = move || {
            for folder in &folders {
                handler(vec![Event {
                    path: folder.clone(),
                    flags: flags::EVENT_IDS_WRAPPED,
                }]);
            }
        };
        std::thread::Builder::new()
            .name("builds-fsevents".into())
            .spawn(move || supervise(&supervisor, opened, open, wrapped))
            .expect("a thread");
        Stream { shared }
    }
}

/// Keeps <shared>'s stream open until it is dropped: opens it when there is
/// none (waiting longer after each refusal), and makes it again when
/// fseventsd is found restarted. <opened>: the first stream
/// is already open. <open> opens a stream from an event id; <wrapped> tells
/// the handler every followed folder needs reading again.
fn supervise(
    shared: &Shared,
    opened: bool,
    open: impl Fn(u64) -> Result<Raw, String>,
    wrapped: impl Fn(),
) {
    let mut backoff = std::time::Duration::from_secs(1);
    let mut seen_id = system::current_event_id();
    let mut following = opened;
    loop {
        if !following {
            let now = system::current_event_id();
            match open(now) {
                Ok(raw) => {
                    crate::output::warn(format!(
                        "{} builds: FSEvents stream open again (event id {now})",
                        crate::text::clock()
                    ));
                    *shared.current.lock().expect("no panic holding it") = Some(raw);
                    following = true;
                    backoff = std::time::Duration::from_secs(1);
                    seen_id = now;
                    wrapped();
                }
                Err(reason) => {
                    crate::output::warn(format!(
                        "{} builds: {reason}; trying again in {}s",
                        crate::text::clock(),
                        backoff.as_secs()
                    ));
                    let wanted = shared.wanted.lock().expect("no panic holding it");
                    let (wanted, _) = shared
                        .wake
                        .wait_timeout_while(wanted, backoff, |w| !w.stop)
                        .expect("no panic holding it");
                    if wanted.stop {
                        return;
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    continue;
                }
            }
        }
        {
            let wanted = shared.wanted.lock().expect("no panic holding it");
            let (wanted, _) = shared
                .wake
                .wait_timeout_while(wanted, WATCH, |w| !w.stop)
                .expect("no panic holding it");
            if wanted.stop {
                *shared.current.lock().expect("no panic holding it") = None;
                return;
            }
        }
        let current = system::current_event_id();
        let why = if current == 0 {
            Some("fseventsd does not answer".to_string())
        } else if current < seen_id {
            Some(format!(
                "fseventsd's event id went back from {seen_id} to {current}: it restarted"
            ))
        } else {
            None
        };
        match why {
            Some(why) => {
                crate::output::warn(format!(
                    "{} builds: FSEvents stream made again: {why}",
                    crate::text::clock()
                ));
                *shared.current.lock().expect("no panic holding it") = None;
                following = false;
            }
            None => seen_id = seen_id.max(current),
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // The supervisor drops the system's stream, which stops it. Nothing
        // here waits: the last reference to the handler may be dropped from
        // the stream's own queue.
        let mut wanted = self.shared.wanted.lock().expect("no panic holding it");
        wanted.stop = true;
        self.shared.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    /// The limit on every wait: a guard against a stuck stream, not the pass
    /// condition.
    const HANG_GUARD: Duration = Duration::from_secs(300);

    /// Tells <freed> when the handler holding it is dropped.
    struct Sentinel(Arc<AtomicBool>, mpsc::Sender<()>);

    impl Drop for Sentinel {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
            let _ = self.1.send(());
        }
    }

    #[test]
    fn a_write_made_before_the_stream_started_is_reported_from_the_id_taken_before_it() {
        let scratch = crate::testing::Scratch::new("fsevents-replay");
        let root = scratch.dir.clone();
        let since = Stream::current_event_id();
        std::fs::write(root.join("early"), b"x").expect("a write");
        let (told, heard) = mpsc::channel();
        let _stream = Stream::start_since(
            since,
            &[root.as_path()],
            &[],
            0.0,
            Box::new(move |events| {
                let _ = told.send(events);
            }),
        );
        loop {
            let events = heard
                .recv_timeout(HANG_GUARD)
                .expect("the earlier write was never reported");
            assert!(
                events.iter().all(|e| e.flags & flags::HISTORY_DONE == 0),
                "the end of the history was handed on"
            );
            if events.iter().any(|e| e.path == root) {
                return;
            }
        }
    }

    #[test]
    fn dropping_the_stream_leaves_the_handler_alone_until_its_running_batch_ends() {
        let scratch = crate::testing::Scratch::new("fsevents-drop");
        let root = scratch.dir.clone();
        let freed = Arc::new(AtomicBool::new(false));
        let (freed_tx, freed_rx) = mpsc::channel();
        let sentinel = Sentinel(freed.clone(), freed_tx);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let seen_freed = Arc::new(AtomicBool::new(false));
        let seen = seen_freed.clone();
        let stream = Stream::start(
            &[root.as_path()],
            &[],
            0.0,
            Box::new(move |_| {
                let _ = entered_tx.send(());
                std::thread::sleep(Duration::from_millis(500));
                // What a freed handler would have lost.
                seen.store(sentinel.0.load(Ordering::SeqCst), Ordering::SeqCst);
                let _ = finished_tx.send(());
            }),
        );
        std::fs::write(root.join("a"), b"x").expect("a write");
        entered_rx.recv_timeout(HANG_GUARD).expect("a batch");
        drop(stream);
        assert!(
            !freed.load(Ordering::SeqCst),
            "the handler was freed while its batch ran"
        );
        finished_rx
            .recv_timeout(HANG_GUARD)
            .expect("the batch ends");
        assert!(!seen_freed.load(Ordering::SeqCst));
        freed_rx
            .recv_timeout(HANG_GUARD)
            .expect("the handler was never freed");
    }

    #[test]
    fn a_stream_the_system_refuses_is_kept_and_stops_when_dropped() {
        let scratch = crate::testing::Scratch::new("fsevents-refused");
        let root = scratch.dir.clone();
        // More than 8 excluded folders: FSEvents refuses the stream, every time.
        let excluded: Vec<PathBuf> = (0..9).map(|n| root.join(format!("x{n}"))).collect();
        let refs: Vec<&Path> = excluded.iter().map(PathBuf::as_path).collect();
        let stream = Stream::start(&[root.as_path()], &refs, 0.0, Box::new(|_| {}));
        drop(stream);
    }
}
