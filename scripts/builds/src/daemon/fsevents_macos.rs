//! macOS's side of fsevents.rs: the system frameworks, declared here by
//! hand, and `Raw`, one stream as FSEvents made it.

use std::ffi::{CStr, CString, c_void};
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Event, Handler, flags};
use crate::clock::Moment;
use crate::core_foundation::{CFRelease, CFStringRef, CfString};

type CFIndex = isize;
type CFAllocatorRef = *const c_void;
type CFArrayRef = *const c_void;
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

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayCreate(
        allocator: CFAllocatorRef,
        values: *const *const c_void,
        count: CFIndex,
        callbacks: *const c_void,
    ) -> CFArrayRef;
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
    let strings: Vec<CfString> = paths
        .iter()
        .map(|path| {
            CfString::new(
                &CString::new(path.as_os_str().as_encoded_bytes()).expect("a path holds no NUL"),
            )
        })
        .collect();
    let pointers: Vec<CFStringRef> = strings.iter().map(CfString::as_ptr).collect();
    // The array keeps its own reference to each string, so the strings are
    // released once it is made.
    unsafe {
        CFArrayCreate(
            std::ptr::null(),
            pointers.as_ptr(),
            pointers.len() as CFIndex,
            &kCFTypeArrayCallBacks as *const c_void,
        )
    }
}

/// The predicate that picks fseventsd's refusals out of the unified log.
/// fseventsd refuses a client past its limits, and says so only there: "too
/// many clients for pid N (limit 512)" past the limit for one process, "too
/// many clients in system (limit 1024)" past the limit for every process
/// together. The FSEvents calls themselves return only NULL or false.
const REFUSALS: &str = "process == \"fseventsd\" AND messageType == error \
                        AND eventMessage CONTAINS \"too many clients\"";

/// Why fseventsd refused this process a stream it asked for at <asked>, from
/// what fseventsd wrote to the unified log since then.
fn fseventsd_says(asked: Moment) -> String {
    let seconds = asked.elapsed().as_secs() + 2;
    let mut command = Command::new("/usr/bin/log");
    command.args([
        "show",
        "--style",
        "compact",
        "--last",
        &format!("{seconds}s"),
        "--predicate",
        REFUSALS,
    ]);
    match crate::host::output_of(command, "/usr/bin/log show", crate::host::Accept::Zero) {
        Ok(text) => {
            let refusals = refusals_of(&text, std::process::id());
            if refusals.is_empty() {
                format!(
                    "fseventsd logged no client limit for this process in the last {seconds}s \
                     (/usr/bin/log show --last 1m --predicate 'process == \"fseventsd\" AND \
                     messageType == error' shows its other errors)"
                )
            } else {
                format!("fseventsd says {}", refusals.join("; "))
            }
        }
        Err(error) => format!("reading fseventsd's reason failed: {error}"),
    }
}

/// The distinct refusals in <text>, `log show --style compact` lines of
/// fseventsd's, that apply to process <pid>: its own limit, and the system's.
fn refusals_of(text: &str, pid: u32) -> Vec<&str> {
    let own = format!("too many clients for pid {pid} ");
    let mut found: Vec<&str> = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find("too many clients") else {
            continue;
        };
        let message = line[at..].trim_end();
        let applies =
            message.starts_with("too many clients in system") || message.starts_with(&own);
        if applies && !found.contains(&message) {
            found.push(message);
        }
    }
    found
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
        let asked = Moment::now();
        let refused = |step: &str| {
            let id = unsafe { FSEventsGetCurrentEventId() };
            let why = if id == 0 {
                "fseventsd does not answer (its current event id is 0)".to_string()
            } else {
                fseventsd_says(asked)
            };
            format!("FSEvents refused {step} (since {since}, fseventsd's event id {id}): {why}")
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

#[cfg(test)]
mod tests {
    use super::refusals_of;

    /// What `log show --style compact` printed for fseventsd's refusals while
    /// a probe opened streams past each limit on macOS 27.
    const LOG: &str = "\
Timestamp               Ty Process[PID:TID]
2026-11-19 05:58:29.101 E  fseventsd[546:82f30f9] [com.apple.fsevents:daemon] too many clients for pid 37874 (limit 512)
2026-11-19 05:58:29.240 E  fseventsd[546:82f30f9] [com.apple.fsevents:daemon] too many clients for pid 378 (limit 512)
2026-11-19 05:59:06.017 E  fseventsd[546:82f30f9] [com.apple.fsevents:daemon] too many clients in system (limit 1024)
2026-11-19 05:59:12.442 E  fseventsd[546:82f30f9] [com.apple.fsevents:daemon] too many clients in system (limit 1024)
";

    #[test]
    fn a_refusal_names_this_process_s_limit_and_the_system_s_once_each() {
        assert_eq!(
            refusals_of(LOG, 378),
            [
                "too many clients for pid 378 (limit 512)",
                "too many clients in system (limit 1024)"
            ]
        );
    }

    #[test]
    fn another_process_s_limit_is_not_this_one_s_reason() {
        let other = LOG
            .lines()
            .filter(|line| !line.contains("in system"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(refusals_of(&other, 3787).is_empty());
    }
}
