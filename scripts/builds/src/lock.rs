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
//! A group of its own is a background group of the terminal, which the kernel
//! stops (SIGTTOU, SIGTTIN) when it sets terminal modes or reads, as Gradle's
//! console does, leaving `wait` waiting for a build that never runs again. So
//! when this process is its terminal's foreground process, the build's group
//! takes the terminal for its run and this process takes it back when the
//! build ends, however it ends. Ctrl-C then reaches the build's group directly.
//! With no terminal on stdin nothing changes.
//!
//! The holder is the process that has the file open: `health` and the
//! dashboard read holder and waiters that way.
//!
//! Waiters go in the order of queue.rs. A takeover sends the holder SIGUSR2:
//! it stops its build's group and, once the build has exited, `run_in_group`
//! says `Ran::TakenOver`; the caller then takes a new place in the queue
//! (`Held::requeue`), straight behind the taker, and runs again.

use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::places::Places;
use crate::queue::{self, Queue, Ticket};
use crate::text::note;

/// The command's process group, for the signal handlers; 0 when none runs.
static GROUP: AtomicI32 = AtomicI32::new(0);
/// Whether this process holds, or waits to hold, its place in a queue and
/// answers a takeover.
static QUEUED: AtomicBool = AtomicBool::new(false);
/// A takeover stopped, or is stopping, the build.
static TAKEN_OVER: AtomicBool = AtomicBool::new(false);

extern "C" fn pass_on(signal: libc::c_int) {
    let group = GROUP.load(Ordering::SeqCst);
    if group > 0 {
        // kill is async-signal-safe.
        unsafe { libc::kill(-group, signal) };
    }
}

/// SIGUSR2: stops the build for a takeover. Outside the time between joining
/// the queue and `Held::release` it does nothing.
extern "C" fn take_over(_: libc::c_int) {
    if !QUEUED.load(Ordering::SeqCst) {
        return;
    }
    TAKEN_OVER.store(true, Ordering::SeqCst);
    pass_on(libc::SIGTERM);
}

pub fn cmd_lock(places: &Places, args: &[String]) -> Result<()> {
    let [repo, command, rest @ ..] = args else {
        fail!("usage: builds lock <repo> <command> [<args>...]");
    };
    let mut held = take(places, repo)?;
    loop {
        let mut start = Command::new(command);
        start.args(rest);
        match run_in_group(start, command)? {
            Ran::Finished => return Ok(()),
            Ran::TakenOver => {
                note(format!("{command} was stopped for a takeover; it runs again after it"));
                held = held.requeue()?;
            }
        }
    }
}

/// <repo>'s app lock, held for the life of this process and every process it
/// starts, unless released first.
pub struct Held {
    lock: libc::c_int,
    ticket: Ticket,
    queue: Queue,
    lock_path: std::path::PathBuf,
}

impl Held {
    /// Frees the lock and lets the next in the queue go. No process this one
    /// started may still run.
    pub fn release(self) {
        // Past its build, a takeover has nothing to stop.
        QUEUED.store(false, Ordering::SeqCst);
        unsafe {
            libc::close(self.lock);
            libc::close(self.ticket.fd);
        }
    }

    /// After a takeover: gives the lock and the place up, takes a new place
    /// where the old one stood, and waits for the lock again.
    pub fn requeue(self) -> Result<Held> {
        TAKEN_OVER.store(false, Ordering::SeqCst);
        let ticket = self.queue.reissue(&self.ticket)?;
        unsafe {
            libc::close(self.lock);
            libc::close(self.ticket.fd);
        }
        self.queue.wait_for_turn(&ticket.name)?;
        Ok(Held { lock: queue::open_locked(&self.lock_path)?, ticket, queue: self.queue, lock_path: self.lock_path })
    }
}

/// Joins <repo>'s queue and waits its turn for the app lock. Fails for a repo
/// with no app checkout.
pub fn take(places: &Places, repo: &str) -> Result<Held> {
    let dir = places.root.join(repo);
    if !places.app_checkout_file(repo).is_file() {
        fail!("{repo} has no app checkout recorded in {}; builds app-checkout records one", dir.display());
    }
    queue::route_repoint_signal();
    unsafe { libc::signal(libc::SIGUSR2, take_over as *const () as libc::sighandler_t) };
    TAKEN_OVER.store(false, Ordering::SeqCst);
    QUEUED.store(true, Ordering::SeqCst);
    let queue = Queue::of(places, repo);
    let (ticket, ahead) = queue.join()?;
    if ahead > 0 {
        note(format!("{ahead} ahead in {repo}'s app queue"));
    }
    queue.wait_for_turn(&ticket.name)?;
    let lock_path = dir.join("app.lock");
    Ok(Held { lock: queue::open_locked(&lock_path)?, ticket, queue, lock_path })
}

/// Whether stdin is a terminal this process is the foreground process of.
fn is_terminal_foreground() -> bool {
    unsafe { libc::isatty(0) == 1 && libc::tcgetpgrp(0) == libc::getpgrp() }
}

/// Makes <group> the terminal's foreground group. Called from a background
/// group, `tcsetpgrp` sends SIGTTOU unless it is blocked, so it is blocked
/// around the call.
fn give_terminal_to(group: libc::pid_t) {
    unsafe {
        let mut blocked = std::mem::zeroed::<libc::sigset_t>();
        let mut before = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGTTOU);
        libc::sigprocmask(libc::SIG_BLOCK, &blocked, &mut before);
        libc::tcsetpgrp(0, group);
        libc::sigprocmask(libc::SIG_SETMASK, &before, std::ptr::null_mut());
    }
}

/// Gives the terminal back to this process's group when dropped, if it was
/// handed to a build.
struct TerminalReturn(bool);

impl Drop for TerminalReturn {
    fn drop(&mut self) {
        if self.0 {
            give_terminal_to(unsafe { libc::getpgrp() });
        }
    }
}

/// How a command run by `run_in_group` ended, when it didn't fail.
#[derive(Debug, PartialEq, Eq)]
pub enum Ran {
    Finished,
    /// A takeover stopped it: it didn't finish and is to run again.
    TakenOver,
}

/// Runs <start> in a process group of its own, passing SIGTERM, SIGINT and
/// SIGHUP on to the whole group, and fails with its exit status, unless a
/// takeover stopped it.
pub fn run_in_group(mut start: Command, command: &str) -> Result<Ran> {
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
    let foreground = is_terminal_foreground();
    start.process_group(0);
    unsafe {
        start.pre_exec(move || {
            // The group exists by now (std makes it before this runs). The
            // terminal goes to it with SIGTTOU blocked, which a background
            // process needs for tcsetpgrp not to stop it.
            if foreground {
                give_terminal_to(libc::getpgrp());
            }
            libc::sigprocmask(libc::SIG_UNBLOCK, &passed, std::ptr::null_mut());
            // The queue's signal is for the waiter, not the build.
            let mut repoint = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut repoint);
            libc::sigaddset(&mut repoint, libc::SIGUSR1);
            libc::sigprocmask(libc::SIG_UNBLOCK, &repoint, std::ptr::null_mut());
            libc::signal(libc::SIGUSR1, libc::SIG_DFL);
            Ok(())
        });
    }
    // Taken back on every way out of here, a failed spawn included.
    let _terminal = TerminalReturn(foreground);
    let mut child = start.spawn().context(|| format!("running {command}"))?;
    let group = child.id() as i32;
    GROUP.store(group, Ordering::SeqCst);
    // A takeover that came before the group was known stops it now.
    if TAKEN_OVER.load(Ordering::SeqCst) {
        unsafe { libc::kill(-group, libc::SIGTERM) };
    }
    unsafe { libc::sigprocmask(libc::SIG_UNBLOCK, &passed, std::ptr::null_mut()) };
    let status = child.wait().context(|| format!("waiting for {command}"))?;
    // The group is gone with its leader's pid free to be reused: no later
    // signal may go to it.
    GROUP.store(0, Ordering::SeqCst);
    match (status.code(), status.signal()) {
        (Some(0), _) => Ok(Ran::Finished),
        _ if TAKEN_OVER.load(Ordering::SeqCst) => Ok(Ran::TakenOver),
        (Some(code), _) => Err(Fail::Exit(code)),
        (None, signal) => Err(Fail::Exit(128 + signal.unwrap_or(0))),
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
