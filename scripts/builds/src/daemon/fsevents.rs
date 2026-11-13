//! macOS FSEvents: a stream of the folders something changed in, under some
//! folders, delivered on a dispatch queue of its own. The system frameworks
//! are declared here by hand; nothing else in the tool needs them.

use std::ffi::{CStr, CString, c_void};
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

type CFIndex = isize;
type CFAllocatorRef = *const c_void;
type CFArrayRef = *const c_void;
type CFStringRef = *const c_void;
type CFTimeInterval = f64;
type FSEventStreamRef = *mut c_void;
type ConstFSEventStreamRef = *const c_void;
type FSEventStreamEventId = u64;
type FSEventStreamEventFlags = u32;
type FSEventStreamCreateFlags = u32;
type DispatchQueue = *mut c_void;

#[repr(C)]
struct FSEventStreamContext {
    version: CFIndex,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type FSEventStreamCallback = extern "C" fn(
    stream: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const FSEventStreamEventFlags,
    ids: *const FSEventStreamEventId,
);

const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(allocator: CFAllocatorRef, text: *const c_char, encoding: u32) -> CFStringRef;
    fn CFArrayCreate(allocator: CFAllocatorRef, values: *const *const c_void, count: CFIndex, callbacks: *const c_void) -> CFArrayRef;
    fn CFRelease(object: *const c_void);
    static kCFTypeArrayCallBacks: c_void;
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventsGetCurrentEventId() -> FSEventStreamEventId;
    fn FSEventStreamCreate(
        allocator: CFAllocatorRef,
        callback: FSEventStreamCallback,
        context: *const FSEventStreamContext,
        paths: CFArrayRef,
        since: FSEventStreamEventId,
        latency: CFTimeInterval,
        flags: FSEventStreamCreateFlags,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: DispatchQueue);
    fn FSEventStreamSetExclusionPaths(stream: FSEventStreamRef, paths: CFArrayRef) -> bool;
    fn FSEventStreamStart(stream: FSEventStreamRef) -> bool;
    fn FSEventStreamFlushSync(stream: FSEventStreamRef);
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> DispatchQueue;
    fn dispatch_release(object: *mut c_void);
    fn dispatch_async_f(queue: DispatchQueue, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

/// What an event says about its path.
pub mod flags {
    pub const MUST_SCAN_SUBDIRS: u32 = 0x1;
    pub const USER_DROPPED: u32 = 0x2;
    pub const KERNEL_DROPPED: u32 = 0x4;
    pub const EVENT_IDS_WRAPPED: u32 = 0x8;
    /// Marks the end of the replayed history; its path is not a change.
    pub const HISTORY_DONE: u32 = 0x10;
    pub const ROOT_CHANGED: u32 = 0x20;
    pub const UNMOUNT: u32 = 0x80;
}

/// One changed path and what the stream says about it.
#[derive(Clone, Debug)]
pub struct Event {
    pub path: PathBuf,
    pub flags: u32,
}

type Handler = Box<dyn Fn(Vec<Event>) + Send + Sync>;

/// A running stream; dropping it stops it.
pub struct Stream {
    stream: FSEventStreamRef,
    queue: DispatchQueue,
    handler: *mut Handler,
}

// The stream and queue are used only through FSEvents' own calls, which are
// safe from any thread.
unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}

extern "C" fn deliver(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const FSEventStreamEventFlags,
    _ids: *const FSEventStreamEventId,
) {
    let handler = unsafe { &*(info as *const Handler) };
    let paths = paths as *const *const c_char;
    let mut events = Vec::with_capacity(count);
    for i in 0..count {
        let path = unsafe { CStr::from_ptr(*paths.add(i)) };
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()));
        let flags = unsafe { *flags.add(i) };
        if flags & self::flags::HISTORY_DONE == 0 {
            events.push(Event { path, flags });
        }
    }
    if !events.is_empty() {
        handler(events);
    }
}

extern "C" fn free_handler(handler: *mut c_void) {
    drop(unsafe { Box::from_raw(handler as *mut Handler) });
}

/// A CFArray of <paths> as CFStrings; the caller releases it.
fn paths_array(paths: &[&Path]) -> CFArrayRef {
    let strings: Vec<CFStringRef> = paths
        .iter()
        .map(|path| {
            let text = CString::new(path.as_os_str().as_encoded_bytes()).expect("a path holds no NUL");
            unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) }
        })
        .collect();
    let array = unsafe {
        CFArrayCreate(std::ptr::null(), strings.as_ptr(), strings.len() as CFIndex, &kCFTypeArrayCallBacks as *const c_void)
    };
    for string in &strings {
        unsafe { CFRelease(*string) };
    }
    array
}

impl Stream {
    /// The id of the latest event the system has recorded. A stream started
    /// from it reports every change made after it was taken.
    pub fn current_event_id() -> u64 {
        unsafe { FSEventsGetCurrentEventId() }
    }

    /// Starts following <folders> as `start_since` does, from the event id
    /// current now.
    pub fn start(folders: &[&Path], excluded: &[&Path], latency: f64, handler: Handler) -> Option<Stream> {
        Self::start_since(Self::current_event_id(), folders, excluded, latency, handler)
    }

    /// Starts following <folders>, leaving out what happens under <excluded>
    /// (at most 8), calling <handler> with each batch of events on a queue of
    /// the stream's own, in order, one batch at a time. The stream replays
    /// every event after <since> (an id from `current_event_id`), so a change
    /// made after it was taken and before the stream went live is reported,
    /// not lost; the replay's end marker is not handed on. With `WatchRoot`,
    /// a change to a followed folder's own path arrives as ROOT_CHANGED.
    pub fn start_since(since: u64, folders: &[&Path], excluded: &[&Path], latency: f64, handler: Handler) -> Option<Stream> {
        let array = paths_array(folders);
        let handler = Box::into_raw(Box::new(handler));
        let context = FSEventStreamContext {
            version: 0,
            info: handler.cast(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        // NoDefer: the first event after a quiet spell comes at once.
        // WatchRoot: a followed folder vanishing or moving is reported.
        let create = 0x2 | 0x4;
        let stream = unsafe { FSEventStreamCreate(std::ptr::null(), deliver, &context, array, since, latency, create) };
        unsafe { CFRelease(array) };
        if stream.is_null() {
            drop(unsafe { Box::from_raw(handler) });
            return None;
        }
        if !excluded.is_empty() {
            let excluded_array = paths_array(excluded);
            let set = unsafe { FSEventStreamSetExclusionPaths(stream, excluded_array) };
            unsafe { CFRelease(excluded_array) };
            if !set {
                unsafe {
                    FSEventStreamInvalidate(stream);
                    FSEventStreamRelease(stream);
                    drop(Box::from_raw(handler));
                }
                return None;
            }
        }
        let queue = unsafe { dispatch_queue_create(c"builds.fsevents".as_ptr(), std::ptr::null()) };
        unsafe {
            FSEventStreamSetDispatchQueue(stream, queue);
            if !FSEventStreamStart(stream) {
                FSEventStreamInvalidate(stream);
                FSEventStreamRelease(stream);
                dispatch_release(queue);
                drop(Box::from_raw(handler));
                return None;
            }
        }
        Some(Stream { stream, queue, handler })
    }

    /// Hands the handler, now, the events the stream holds back for its
    /// latency. Events still on their way from the kernel come after.
    pub fn flush(&self) {
        unsafe { FSEventStreamFlushSync(self.stream) };
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
            // A batch may be running on the queue right now, or waiting in it,
            // and holds the handler. Stopping the stream does not wait for
            // either, so the handler is freed from the queue itself, after
            // them (the queue runs one block at a time). Waiting here instead
            // would deadlock when the handler's own last reference drops the
            // stream on the queue.
            dispatch_async_f(self.queue, self.handler.cast(), free_handler);
            dispatch_release(self.queue);
        }
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
        )
        .expect("a stream");
        loop {
            let events = heard.recv_timeout(HANG_GUARD).expect("the earlier write was never reported");
            assert!(events.iter().all(|e| e.flags & flags::HISTORY_DONE == 0), "the end of the history was handed on");
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
        )
        .expect("a stream");
        std::fs::write(root.join("a"), b"x").expect("a write");
        entered_rx.recv_timeout(HANG_GUARD).expect("a batch");
        drop(stream);
        assert!(!freed.load(Ordering::SeqCst), "the handler was freed while its batch ran");
        finished_rx.recv_timeout(HANG_GUARD).expect("the batch ends");
        assert!(!seen_freed.load(Ordering::SeqCst));
        freed_rx.recv_timeout(HANG_GUARD).expect("the handler was never freed");
    }
}
