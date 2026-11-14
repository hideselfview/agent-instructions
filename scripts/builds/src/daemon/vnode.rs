//! Tells when something is added to, removed from or renamed in a folder, or
//! the folder itself is renamed, deleted or unmounted, through a kqueue watch
//! on the folder.
//!
//! The kernel records each change in the kqueue as it happens, so a change
//! made after `Watch::start` returns is always reported, however late the
//! thread that reads the kqueue runs. FSEvents gives no such bound: on a
//! loaded machine it delivered a folder's change from 0.1 s to over 30 s
//! late, and in a test run reported none of a rename for two minutes. The
//! tracker learns that its root was replaced or came back from this watch
//! on the root's parent, so it can not wait on FSEvents for that.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

/// What a watch reports: entries added, removed or renamed in the folder, and
/// the folder itself renamed, deleted or unmounted.
const CHANGES: u32 =
    libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE | libc::NOTE_LINK;

/// The user event that ends the reader thread.
const STOP: libc::uintptr_t = 0;

/// A running watch; dropping it ends it.
pub struct Watch {
    queue: Arc<OwnedFd>,
}

fn event(ident: libc::uintptr_t, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    libc::kevent {
        ident,
        filter,
        flags,
        fflags,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}

fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

impl Watch {
    /// Calls <changed> on a thread of the watch's own after every change made
    /// to <folder> from when this returns.
    pub fn start(folder: &Path, changed: impl Fn() + Send + 'static) -> io::Result<Watch> {
        let path = CString::new(folder.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path holds no NUL"))?;
        let dir = owned(unsafe { libc::open(path.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) })?;
        let queue = Arc::new(owned(unsafe { libc::kqueue() })?);
        let registered = [
            event(
                dir.as_raw_fd() as libc::uintptr_t,
                libc::EVFILT_VNODE,
                libc::EV_ADD | libc::EV_CLEAR,
                CHANGES,
            ),
            event(STOP, libc::EVFILT_USER, libc::EV_ADD | libc::EV_CLEAR, 0),
        ];
        let set = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                registered.as_ptr(),
                registered.len() as libc::c_int,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if set < 0 {
            return Err(io::Error::last_os_error());
        }
        let reading = queue.clone();
        let watched = folder.display().to_string();
        std::thread::Builder::new()
            .name("builds-vnode".into())
            .spawn(move || {
                // The folder stays open for as long as the watch runs.
                let _dir = dir;
                loop {
                    let mut happened = event(0, 0, 0, 0);
                    let got = unsafe {
                        libc::kevent(
                            reading.as_raw_fd(),
                            std::ptr::null(),
                            0,
                            &mut happened,
                            1,
                            std::ptr::null(),
                        )
                    };
                    if got < 0 {
                        if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                            continue;
                        }
                        eprintln!(
                            "builds: watching {watched}: {}; no longer watched",
                            io::Error::last_os_error()
                        );
                        return;
                    }
                    if happened.filter == libc::EVFILT_USER {
                        return;
                    }
                    changed();
                }
            })?;
        Ok(Watch { queue })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let stop = event(STOP, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER);
        let sent = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                &stop,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if sent < 0 {
            eprintln!(
                "builds: stopping a folder watch: {}; its thread stays",
                io::Error::last_os_error()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::sync::mpsc;
    use std::time::Duration;

    /// The limit on every wait: a guard against a stuck watch, not the pass
    /// condition.
    const HANG_GUARD: Duration = Duration::from_secs(300);

    #[test]
    fn a_change_made_right_after_the_watch_started_is_reported() {
        let scratch = Scratch::new("vnode");
        let folder = scratch.dir.join("watched");
        std::fs::create_dir_all(folder.join("a")).unwrap();
        let (told, heard) = mpsc::channel();
        let _watch = Watch::start(&folder, move || {
            let _ = told.send(());
        })
        .unwrap();
        std::fs::rename(folder.join("a"), folder.join("b")).unwrap();
        heard
            .recv_timeout(HANG_GUARD)
            .expect("the rename was never reported");
        std::fs::create_dir(folder.join("c")).unwrap();
        heard
            .recv_timeout(HANG_GUARD)
            .expect("the new folder was never reported");
    }

    #[test]
    fn the_folder_itself_renamed_is_reported() {
        let scratch = Scratch::new("vnode-self");
        let folder = scratch.dir.join("watched");
        std::fs::create_dir_all(&folder).unwrap();
        let (told, heard) = mpsc::channel();
        let _watch = Watch::start(&folder, move || {
            let _ = told.send(());
        })
        .unwrap();
        std::fs::rename(&folder, scratch.dir.join("moved")).unwrap();
        heard
            .recv_timeout(HANG_GUARD)
            .expect("the rename was never reported");
    }

    #[test]
    fn a_dropped_watch_reports_nothing_more() {
        let scratch = Scratch::new("vnode-drop");
        let folder = scratch.dir.join("watched");
        std::fs::create_dir_all(&folder).unwrap();
        let (told, heard) = mpsc::channel();
        let watch = Watch::start(&folder, move || {
            let _ = told.send(());
        })
        .unwrap();
        drop(watch);
        // The reader thread ends and drops its sender: the channel closes.
        assert!(matches!(
            heard.recv_timeout(HANG_GUARD),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
}
