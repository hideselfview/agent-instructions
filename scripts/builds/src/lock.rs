//! `builds lock <repo> <command> [<args>...]`: runs a command holding the
//! repo's app lock, <root>/<repo>/app.lock.
//!
//! The lock lives as long as any process of the build does, so no other
//! command can start in the app checkout while one of them still runs. `lockf`
//! held it in its own process alone: killing `lockf` freed the lock while the
//! command it had started kept building, and the next waiter started a second
//! build in the same checkout. Here the lock's file descriptor is open without
//! close-on-exec, so the command and every process it starts hold it too, and
//! the kernel frees the lock only when the last of them exits.
//!
//! The command runs in a process group of its own, and SIGTERM, SIGINT and
//! SIGHUP sent to this process are passed on to that whole group, so killing
//! the holder stops its build; the lock goes once the build has exited.
//!
//! A waiter blocks inside `open` (O_EXLOCK), as `lockf` does, so the process
//! that has the file open is the one holding it: `health` and the dashboard
//! read holder and waiters that way.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::places::Places;

/// The command's process group, for the signal handler; 0 before it starts.
static GROUP: AtomicI32 = AtomicI32::new(0);

extern "C" fn pass_on(signal: libc::c_int) {
    let group = GROUP.load(Ordering::SeqCst);
    if group > 0 {
        // kill is async-signal-safe.
        unsafe { libc::kill(-group, signal) };
    }
}

/// Opens <path>, creating it, and waits inside `open` until it holds the
/// file's exclusive lock. The descriptor is left open without close-on-exec
/// for the life of this process and passed to every process it starts.
fn open_locked(path: &Path) -> Result<libc::c_int> {
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("a path holds no NUL");
    loop {
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_CREAT | libc::O_EXLOCK, 0o644) };
        if fd >= 0 {
            return Ok(fd);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            fail!("locking {}: {error}", path.display());
        }
    }
}

pub fn cmd_lock(places: &Places, args: &[String]) -> Result<()> {
    let [repo, command, rest @ ..] = args else {
        fail!("usage: builds lock <repo> <command> [<args>...]");
    };
    let dir = places.root.join(repo);
    if !places.app_checkout_file(repo).is_file() {
        fail!("{repo} has no app checkout recorded in {}; builds app-checkout records one", dir.display());
    }
    let path = dir.join("app.lock");
    open_locked(&path)?;
    // The signals stay blocked until the group is known, so one sent while
    // the command starts is passed on rather than lost; the command starts
    // with them unblocked.
    let mut passed = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    unsafe {
        libc::sigemptyset(&mut passed);
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut passed, signal);
            libc::signal(signal, pass_on as *const () as libc::sighandler_t);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &passed, std::ptr::null_mut());
    }
    let mut start = Command::new(command);
    start.args(rest).process_group(0);
    unsafe {
        start.pre_exec(move || {
            libc::sigprocmask(libc::SIG_UNBLOCK, &passed, std::ptr::null_mut());
            Ok(())
        });
    }
    let mut child = start.spawn().context(|| format!("running {command}"))?;
    GROUP.store(child.id() as i32, Ordering::SeqCst);
    unsafe { libc::sigprocmask(libc::SIG_UNBLOCK, &passed, std::ptr::null_mut()) };
    let status = child.wait().context(|| format!("waiting for {command}"))?;
    match (status.code(), status.signal()) {
        (Some(0), _) => Ok(()),
        (Some(code), _) => Err(Fail::Exit(code)),
        (None, signal) => Err(Fail::Exit(128 + signal.unwrap_or(0))),
    }
}
