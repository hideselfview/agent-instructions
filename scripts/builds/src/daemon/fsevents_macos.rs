//! macOS's side of fsevents.rs: the system frameworks, declared here by
//! hand, and `Raw`, one stream as FSEvents made it.

use std::ffi::{CStr, CString, c_void};
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::{Event, Handler, flags};

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
    fn CFStringCreateWithCString(
        allocator: CFAllocatorRef,
        text: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFArrayCreate(
        allocator: CFAllocatorRef,
        values: *const *const c_void,
        count: CFIndex,
        callbacks: *const c_void,
    ) -> CFArrayRef;
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
    fn dispatch_async_f(
        queue: DispatchQueue,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

/// The id of the latest event the system has recorded; 0 when fseventsd
/// does not answer.
pub fn current_event_id() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}

/// One FSEvents stream as the system made it; dropping it stops it.
pub struct Raw {
    stream: FSEventStreamRef,
    queue: DispatchQueue,
    handler: *mut Handler,
}

// The stream and queue are used only through FSEvents' own calls, which are
// safe from any thread.
unsafe impl Send for Raw {}
unsafe impl Sync for Raw {}

extern "C" fn deliver(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const FSEventStreamEventFlags,
    ids: *const FSEventStreamEventId,
) {
    let handler = unsafe { &*(info as *const Handler) };
    let paths = paths as *const *const c_char;
    let mut events = Vec::with_capacity(count);
    for i in 0..count {
        let path = unsafe { CStr::from_ptr(*paths.add(i)) };
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()));
        let flags = unsafe { *flags.add(i) };
        if flags & self::flags::HISTORY_DONE == 0 {
            let id = unsafe { *ids.add(i) };
            events.push(Event { path, flags, id });
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
            let text =
                CString::new(path.as_os_str().as_encoded_bytes()).expect("a path holds no NUL");
            unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) }
        })
        .collect();
    let array = unsafe {
        CFArrayCreate(
            std::ptr::null(),
            strings.as_ptr(),
            strings.len() as CFIndex,
            &kCFTypeArrayCallBacks as *const c_void,
        )
    };
    for string in &strings {
        unsafe { CFRelease(*string) };
    }
    array
}

impl Raw {
    /// Starts following <folders>, leaving out what happens under <excluded>
    /// (at most 8), replaying every event after <since>, and says why when the
    /// system refuses.
    pub fn open(
        since: u64,
        folders: &[&Path],
        excluded: &[&Path],
        latency: f64,
        handler: Handler,
    ) -> Result<Raw, String> {
        let refused = |step: &str| {
            let id = unsafe { FSEventsGetCurrentEventId() };
            let hint = if id == 0 {
                "; fseventsd does not answer (its current event id is 0)"
            } else {
                ""
            };
            format!("FSEvents refused {step} (since {since}, fseventsd's event id {id}){hint}")
        };
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
        let stream = unsafe {
            FSEventStreamCreate(
                std::ptr::null(),
                deliver,
                &context,
                array,
                since,
                latency,
                create,
            )
        };
        unsafe { CFRelease(array) };
        if stream.is_null() {
            drop(unsafe { Box::from_raw(handler) });
            return Err(refused("to create the stream"));
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
                return Err(refused("the excluded folders"));
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
                return Err(refused("to start the stream"));
            }
        }
        Ok(Raw {
            stream,
            queue,
            handler,
        })
    }

    pub fn flush(&self) {
        unsafe { FSEventStreamFlushSync(self.stream) };
    }
}

impl Drop for Raw {
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
