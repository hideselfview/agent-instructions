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
//! The holder is the process that has the file open: `health` and the
//! dashboard read holder and waiters that way.
//!
//! Waiters go in the order they came. Each joins <root>/<repo>/app.queue with
//! the next numbered ticket, a file it keeps locked for its whole life, as it
//! does the lock, and waits on the ticket before it, blocked inside `open`
//! (O_EXLOCK). A ticket whose process is gone frees at once, so the waiter
//! behind it takes it away and waits on the one before that: a killed waiter
//! never lets the next one past the holder. The file lock alone wakes waiters
//! in no set order.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::places::Places;
use crate::text::note;

/// The command's process group, for the signal handler; 0 before it starts.
static GROUP: AtomicI32 = AtomicI32::new(0);

extern "C" fn pass_on(signal: libc::c_int) {
    let group = GROUP.load(Ordering::SeqCst);
    if group > 0 {
        // kill is async-signal-safe.
        unsafe { libc::kill(-group, signal) };
    }
}

/// Opens <path> with <flags> and an exclusive lock, waiting inside `open`
/// unless <flags> says not to. The descriptor is left open without
/// close-on-exec for the life of this process and passed to every process it
/// starts.
fn open_with(path: &Path, flags: libc::c_int) -> std::io::Result<libc::c_int> {
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("a path holds no NUL");
    loop {
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_EXLOCK | flags, 0o644) };
        if fd >= 0 {
            return Ok(fd);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn open_locked(path: &Path) -> Result<libc::c_int> {
    open_with(path, libc::O_CREAT).context(|| format!("locking {}", path.display()))
}

/// The tickets in <queue>, lowest first.
fn tickets(queue: &Path) -> Result<Vec<u64>> {
    let entries = std::fs::read_dir(queue).context(|| format!("reading {}", queue.display()))?;
    let mut numbers: Vec<u64> = entries.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect();
    numbers.sort_unstable();
    Ok(numbers)
}

/// Takes the next ticket in <queue>, returning its number, its descriptor, and
/// how many live tickets are before it.
fn join_queue(queue: &Path) -> Result<(u64, libc::c_int, usize)> {
    std::fs::create_dir_all(queue).context(|| format!("creating {}", queue.display()))?;
    let guard = open_locked(&queue.join(".next"))?;
    let taken = tickets(queue)?;
    let number = taken.last().map_or(1, |last| last + 1);
    let ticket = open_locked(&queue.join(number.to_string()))?;
    unsafe { libc::close(guard) };
    let ahead = taken.iter().filter(|before| is_held(&queue.join(before.to_string()))).count();
    Ok((number, ticket, ahead))
}

/// Whether a process holds the ticket at <path>.
fn is_held(path: &Path) -> bool {
    match open_with(path, libc::O_NONBLOCK) {
        Ok(fd) => {
            unsafe { libc::close(fd) };
            false
        }
        Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
    }
}

/// Waits until no ticket below <number> is left in <queue>.
fn wait_for_turn(queue: &Path, number: u64) -> Result<()> {
    while let Some(before) = tickets(queue)?.into_iter().rfind(|ticket| *ticket < number) {
        let path = queue.join(before.to_string());
        match open_with(&path, 0) {
            Ok(fd) => {
                let removed = std::fs::remove_file(&path);
                unsafe { libc::close(fd) };
                removed.context(|| format!("removing {}", path.display()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => fail!("waiting on {}: {error}", path.display()),
        }
    }
    Ok(())
}

pub fn cmd_lock(places: &Places, args: &[String]) -> Result<()> {
    let [repo, command, rest @ ..] = args else {
        fail!("usage: builds lock <repo> <command> [<args>...]");
    };
    take(places, repo)?;
    let mut start = Command::new(command);
    start.args(rest);
    run_in_group(start, command)
}

/// <repo>'s app lock, held for the life of this process and every process it
/// starts, unless released first.
pub struct Held {
    lock: libc::c_int,
    ticket: libc::c_int,
}

impl Held {
    /// Frees the lock and lets the next in the queue go. No process this one
    /// started may still run.
    pub fn release(self) {
        unsafe {
            libc::close(self.lock);
            libc::close(self.ticket);
        }
    }
}

/// Joins <repo>'s queue and waits its turn for the app lock. Fails for a repo
/// with no app checkout.
pub fn take(places: &Places, repo: &str) -> Result<Held> {
    let dir = places.root.join(repo);
    if !places.app_checkout_file(repo).is_file() {
        fail!("{repo} has no app checkout recorded in {}; builds app-checkout records one", dir.display());
    }
    let queue = dir.join("app.queue");
    let (number, ticket, ahead) = join_queue(&queue)?;
    if ahead > 0 {
        note(format!("{ahead} ahead in {repo}'s app queue"));
    }
    wait_for_turn(&queue, number)?;
    Ok(Held { lock: open_locked(&dir.join("app.lock"))?, ticket })
}

/// Runs <start> in a process group of its own, passing SIGTERM, SIGINT and
/// SIGHUP on to the whole group, and fails with its exit status.
pub fn run_in_group(mut start: Command, command: &str) -> Result<()> {
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
    start.process_group(0);
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

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
