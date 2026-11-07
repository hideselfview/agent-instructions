//! The line for a repo's app lock: <root>/<repo>/app.queue.
//!
//! `order` names one ticket per line, next first; the holder's is at the head.
//! Every change to it happens under a short guard lock, `.next`. A ticket is a
//! file named `<pid>-<n>` (n counts that process's tickets), held locked for
//! the life of its process and of every process that process starts, and
//! holding nothing else: the name carries the pid.
//!
//! A waiter blocks, in a thread inside `open` (O_EXLOCK), on the ticket of the
//! entry before it. Getting that lock means the ticket's owner is gone, so the
//! waiter takes the entry out of the order and looks again: a waiter that dies
//! in line never lets anyone past the holder. SIGUSR1 means the order changed:
//! the waiter's own thread waits in a kqueue for that signal and for its
//! blocked threads, registered before it first reads the order, so a signal
//! is counted whether or not the waiter is blocked at that moment (a lost
//! signal would leave a waiter on the wrong ticket, and a reordering can turn
//! that into a cycle of waiters). The waiter then reads the order again and
//! starts a thread on the new predecessor; a thread left on the old one gives
//! any ticket it gets back at once, and ends when that ticket frees.
//!
//! `builds queue <repo>` shows the line, `builds queue <repo> top <pid>` moves
//! a waiter to right behind the holder, and `builds takeover <repo> [<pid>]`
//! stops the holder's build and puts a waiter ahead of it (lock.rs).

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::procs::{self, Processes};

/// How many tickets this process has made.
static TICKETS: AtomicU64 = AtomicU64::new(0);

/// SIGUSR1 reaches the waiter through its kqueue only: ignored, and blocked
/// in the thread that waits. `run_in_group` resets both in what it starts.
pub fn route_repoint_signal() {
    unsafe {
        libc::signal(libc::SIGUSR1, libc::SIG_IGN);
        let mut set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGUSR1);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// Opens <path> with <flags> and an exclusive lock, waiting inside `open`
/// unless <flags> says not to. The descriptor is left open without
/// close-on-exec unless <flags> says so: a ticket or lock passes to every
/// process this one starts.
pub fn open_with(path: &Path, flags: libc::c_int) -> std::io::Result<libc::c_int> {
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

pub fn open_locked(path: &Path) -> Result<libc::c_int> {
    open_with(path, libc::O_CREAT).context(|| format!("locking {}", path.display()))
}

/// Whether a process holds the lock on the file at <path>.
fn is_held(path: &Path) -> Result<bool> {
    match open_with(path, libc::O_NONBLOCK) {
        Ok(fd) => {
            unsafe { libc::close(fd) };
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("checking {}: {error}", path.display()),
    }
}

/// The pid in a ticket's name.
pub fn pid_of(ticket: &str) -> Option<i32> {
    ticket.split_once('-')?.0.parse().ok()
}

/// One ticket of this process: held locked until its descriptor closes.
pub struct Ticket {
    pub name: String,
    pub fd: libc::c_int,
}

/// <repo>'s queue folder.
pub struct Queue {
    dir: PathBuf,
}

/// The guard lock, held until dropped. It is not passed to processes this one
/// starts.
struct Guard(libc::c_int);

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

impl Queue {
    pub fn of(places: &Places, repo: &str) -> Queue {
        Queue { dir: places.root.join(repo).join("app.queue") }
    }

    pub fn in_repo_dir(repo_dir: &Path) -> Queue {
        Queue { dir: repo_dir.join("app.queue") }
    }

    fn ticket_path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn guard(&self) -> Result<Guard> {
        std::fs::create_dir_all(&self.dir).context(|| format!("creating {}", self.dir.display()))?;
        let path = self.dir.join(".next");
        open_with(&path, libc::O_CREAT | libc::O_CLOEXEC)
            .map(Guard)
            .context(|| format!("locking {}", path.display()))
    }

    /// The tickets in line, next first. A write replaces the file whole, so a
    /// read without the guard sees one order or the other.
    pub fn order(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("order")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    fn write_order(&self, order: &[String]) -> Result<()> {
        let part = self.dir.join("order.part");
        let text: String = order.iter().map(|name| format!("{name}\n")).collect();
        std::fs::write(&part, text).context(|| format!("writing {}", part.display()))?;
        let order_file = self.dir.join("order");
        std::fs::rename(&part, &order_file).context(|| format!("replacing {}", order_file.display()))
    }

    /// The pid of each ticket in line after the head, next first.
    pub fn waiting(&self) -> Vec<i32> {
        self.order().iter().skip(1).filter_map(|name| pid_of(name)).collect()
    }

    /// Makes this process's next ticket, locked, and puts it last in line;
    /// returns it with how many were in line before it.
    pub fn join(&self) -> Result<(Ticket, usize)> {
        let _guard = self.guard()?;
        let ticket = self.new_ticket()?;
        let mut order = self.order();
        let ahead = order.len();
        order.push(ticket.name.clone());
        self.write_order(&order)?;
        Ok((ticket, ahead))
    }

    /// A fresh ticket, locked, not yet in the order. Needs the guard.
    fn new_ticket(&self) -> Result<Ticket> {
        let number = TICKETS.fetch_add(1, Ordering::SeqCst) + 1;
        let name = format!("{}-{number}", std::process::id());
        let fd = open_locked(&self.ticket_path(&name))?;
        Ok(Ticket { name, fd })
    }

    /// Gives <old> up for a new ticket that takes its place in line; <old>'s
    /// descriptor is the caller's to close.
    pub fn reissue(&self, old: &Ticket) -> Result<Ticket> {
        let _guard = self.guard()?;
        let new = self.new_ticket()?;
        let mut order = self.order();
        match order.iter_mut().find(|name| **name == old.name) {
            Some(entry) => *entry = new.name.clone(),
            None => fail!("{} is not in the queue", old.name),
        }
        self.write_order(&order)?;
        self.unlink(&old.name)?;
        Ok(new)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let path = self.ticket_path(name);
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                fail!("removing {}: {error}", path.display())
            }
            _ => Ok(()),
        }
    }

    /// Takes <name> out of the order and removes its file: its owner is gone.
    fn drop_ticket(&self, name: &str) -> Result<()> {
        let _guard = self.guard()?;
        let mut order = self.order();
        if order.iter().any(|entry| entry == name) {
            order.retain(|entry| entry != name);
            self.write_order(&order)?;
        }
        self.unlink(name)
    }

    /// Waits until <mine> is first in the order.
    pub fn wait_for_turn(&self, mine: &str) -> Result<()> {
        self.wait_for_turn_with(mine, || {})
    }

    /// `wait_for_turn`, calling <after_read> once the order is read and the
    /// wait on its predecessor started, before the process waits for an
    /// event: where a signal must not be lost.
    fn wait_for_turn_with(&self, mine: &str, mut after_read: impl FnMut()) -> Result<()> {
        route_repoint_signal();
        // Before the first read: a signal sent for a change this read
        // misses is counted from here on.
        let wake = Wake::new()?;
        let shared = Arc::new(Mutex::new(Shared { current: 0, open: true }));
        let mut generation = 0;
        let mut waiting: Option<(u64, String)> = None;
        let result = (|| -> Result<()> {
            loop {
                let order = self.order();
                let Some(at) = order.iter().position(|name| name == mine) else {
                    fail!("{mine} is no longer in {}", self.dir.join("order").display());
                };
                if at == 0 {
                    return Ok(());
                }
                let before = &order[at - 1];
                if waiting.as_ref().is_none_or(|(_, name)| name != before) {
                    generation += 1;
                    shared.lock().expect("no panic holding it").current = generation;
                    wake.block_on(self.ticket_path(before), generation, shared.clone())?;
                    waiting = Some((generation, before.clone()));
                }
                after_read();
                if wake.wait()?.ready {
                    while let Some((from, code)) = wake.message() {
                        let Some(name) = waiting.as_ref().filter(|(current, _)| *current == from).map(|(_, name)| name.clone()) else {
                            // A wait on a ticket since given up: it holds a
                            // ticket for this instant only.
                            if code >= 0 {
                                unsafe { libc::close(code) };
                            }
                            continue;
                        };
                        if code >= 0 {
                            let dropped = self.drop_ticket(&name);
                            unsafe { libc::close(code) };
                            dropped?;
                        } else if -code == libc::ENOENT {
                            self.drop_ticket(&name)?;
                        } else {
                            fail!("waiting on {}: {}", self.ticket_path(&name).display(), std::io::Error::from_raw_os_error(-code));
                        }
                        waiting = None;
                    }
                }
            }
        })();
        // No thread of this wait reports from here on; what was reported and
        // not read is let go.
        shared.lock().expect("no panic holding it").open = false;
        while let Some((_, code)) = wake.message() {
            if code >= 0 {
                unsafe { libc::close(code) };
            }
        }
        result
    }

    /// Moves the ticket of <pid> to right behind the holder, which is first.
    fn move_to_top(&self, pid: i32) -> Result<()> {
        let _guard = self.guard()?;
        let mut order = self.order();
        let Some(at) = order.iter().position(|name| pid_of(name) == Some(pid)) else {
            fail!("{pid} is not in the queue");
        };
        if at == 0 {
            fail!("{pid} is first in the queue: it holds the lock or is about to");
        }
        let name = order.remove(at);
        order.insert(1.min(order.len()), name);
        self.write_order(&order)
    }
}

/// What the threads blocked in `open` and the wait agree on.
struct Shared {
    /// The wait whose result is wanted; a thread of another one lets its
    /// ticket go.
    current: u64,
    /// False once the wait has ended.
    open: bool,
}

/// A wait's kqueue: it holds SIGUSR1 (counted from `new` on) and the read end
/// of the pipe its threads report through.
struct Wake {
    kqueue: libc::c_int,
    read: libc::c_int,
    write: libc::c_int,
}

/// What `Wake::wait` woke for. Only a report needs reading; the signal alone
/// makes the caller read the order again.
struct Woke {
    ready: bool,
}

const MESSAGE: usize = 16;

fn change(ident: usize, filter: i16, flags: u16) -> libc::kevent {
    libc::kevent { ident, filter, flags, fflags: 0, data: 0, udata: std::ptr::null_mut() }
}

impl Wake {
    fn new() -> Result<Wake> {
        let mut ends = [0; 2];
        let kqueue = unsafe { libc::kqueue() };
        if kqueue < 0 || unsafe { libc::pipe(ends.as_mut_ptr()) } < 0 {
            fail!("making a queue to wait in: {}", std::io::Error::last_os_error());
        }
        unsafe {
            libc::fcntl(kqueue, libc::F_SETFD, libc::FD_CLOEXEC);
            for end in ends {
                libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            libc::fcntl(ends[0], libc::F_SETFL, libc::O_NONBLOCK);
        }
        let wake = Wake { kqueue, read: ends[0], write: ends[1] };
        let changes = [
            change(libc::SIGUSR1 as usize, libc::EVFILT_SIGNAL, libc::EV_ADD | libc::EV_CLEAR),
            change(wake.read as usize, libc::EVFILT_READ, libc::EV_ADD),
        ];
        let set = unsafe { libc::kevent(kqueue, changes.as_ptr(), 2, std::ptr::null_mut(), 0, std::ptr::null()) };
        if set < 0 {
            fail!("registering what to wait for: {}", std::io::Error::last_os_error());
        }
        Ok(wake)
    }

    /// Starts a thread that waits for the lock on <path> and reports the
    /// result for wait <generation>, unless that is no longer the current one.
    fn block_on(&self, path: PathBuf, generation: u64, shared: Arc<Mutex<Shared>>) -> Result<()> {
        let write = unsafe { libc::fcntl(self.write, libc::F_DUPFD_CLOEXEC, 0) };
        if write < 0 {
            fail!("starting a wait: {}", std::io::Error::last_os_error());
        }
        std::thread::spawn(move || {
            // Close-on-exec: this descriptor only waits, it is never held.
            let result = open_with(&path, libc::O_CLOEXEC);
            let code = match &result {
                Ok(fd) => *fd,
                Err(error) => -error.raw_os_error().unwrap_or(libc::EIO),
            };
            let sent = {
                let shared = shared.lock().expect("no panic holding it");
                if shared.open && shared.current == generation {
                    let mut message = [0u8; MESSAGE];
                    message[..8].copy_from_slice(&generation.to_le_bytes());
                    message[8..12].copy_from_slice(&code.to_le_bytes());
                    unsafe { libc::write(write, message.as_ptr().cast(), MESSAGE) == MESSAGE as isize }
                } else {
                    false
                }
            };
            unsafe { libc::close(write) };
            if let (false, Ok(fd)) = (sent, result) {
                unsafe { libc::close(fd) };
            }
        });
        Ok(())
    }

    /// Blocks until the signal came or a thread reported.
    fn wait(&self) -> Result<Woke> {
        let mut events = [change(0, 0, 0); 4];
        loop {
            let count = unsafe { libc::kevent(self.kqueue, std::ptr::null(), 0, events.as_mut_ptr(), 4, std::ptr::null()) };
            if count >= 0 {
                let ready = events[..count as usize].iter().any(|event| event.filter == libc::EVFILT_READ);
                return Ok(Woke { ready });
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                fail!("waiting: {error}");
            }
        }
    }

    /// The next report: the wait it is for and what its thread got, a
    /// descriptor or the negative of an error number.
    fn message(&self) -> Option<(u64, i32)> {
        let mut message = [0u8; MESSAGE];
        let read = unsafe { libc::read(self.read, message.as_mut_ptr().cast(), MESSAGE) };
        (read == MESSAGE as isize).then(|| {
            let wait = u64::from_le_bytes(message[..8].try_into().expect("8 bytes"));
            (wait, i32::from_le_bytes(message[8..12].try_into().expect("4 bytes")))
        })
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.kqueue);
            libc::close(self.read);
            libc::close(self.write);
        }
    }
}

/// Whether <pid> is a `builds` process: a signal goes only to one, since the
/// pid in a ticket may be a stranger's by now.
fn is_builds(processes: &Processes, pid: i32) -> bool {
    processes.get(pid).is_some_and(|process| process.name == "builds")
}

/// Sends <signal> to each of <pids> that is a `builds` process, not this one.
fn signal_all(pids: &[i32], signal: libc::c_int) {
    let processes = Processes::read();
    for pid in pids {
        if *pid != std::process::id() as i32 && is_builds(&processes, *pid) {
            unsafe { libc::kill(*pid, signal) };
        }
    }
}

fn pids_in(order: &[String]) -> Vec<i32> {
    order.iter().filter_map(|name| pid_of(name)).collect()
}

fn check_repo(places: &Places, repo: &str) -> Result<()> {
    if repo.is_empty() || repo.starts_with('.') || repo.contains('/') {
        fail!("{repo:?} is not a repo's name");
    }
    if !places.app_checkout_file(repo).is_file() {
        fail!("{repo} has no app checkout recorded; builds app-checkout records one");
    }
    Ok(())
}

/// Moves the waiter <pid> to right behind the holder and tells every ticket's
/// process to read the order again.
pub fn top(places: &Places, repo: &str, pid: i32) -> Result<()> {
    check_repo(places, repo)?;
    let queue = Queue::of(places, repo);
    queue.move_to_top(pid)?;
    signal_all(&pids_in(&queue.order()), libc::SIGUSR1);
    Ok(())
}

/// Puts the waiter <pid> (default the first) ahead of the holder, stops the
/// holder's build, and tells every waiter to read the order again. The
/// stopped build goes straight behind the waiter and runs again by itself.
/// With nothing holding the lock, the same as `top`.
pub fn takeover(places: &Places, repo: &str, pid: Option<i32>) -> Result<()> {
    check_repo(places, repo)?;
    let queue = Queue::of(places, repo);
    let holder = {
        let _guard = queue.guard()?;
        let mut order = queue.order();
        let taker = match pid {
            Some(pid) => order.iter().position(|name| pid_of(name) == Some(pid)),
            None => (order.len() > 1).then_some(1),
        };
        let Some(taker) = taker else {
            match pid {
                Some(pid) => fail!("{pid} is not in the queue"),
                None => fail!("nobody waits in {repo}'s queue"),
            }
        };
        if taker == 0 {
            fail!("{} is first in the queue: it holds the lock or is about to", pid.unwrap_or_default());
        }
        let holding = is_held(&places.root.join(repo).join("app.lock"))?;
        let name = order.remove(taker);
        if !holding {
            order.insert(1.min(order.len()), name);
            queue.write_order(&order)?;
            None
        } else {
            let holder = order[0].clone();
            let Some(holder_pid) = pid_of(&holder).filter(|pid| is_builds(&Processes::read(), *pid)) else {
                fail!("the holder's `builds` is gone, so its build can't be told to stop; kill its processes by pid");
            };
            order.insert(0, name);
            queue.write_order(&order)?;
            Some(holder_pid)
        }
    };
    let order = queue.order();
    let waiters: Vec<i32> = pids_in(&order).into_iter().filter(|pid| Some(*pid) != holder).collect();
    signal_all(&waiters, libc::SIGUSR1);
    if let Some(holder) = holder {
        signal_all(&[holder], libc::SIGUSR2);
    }
    Ok(())
}

/// `builds queue <repo> [top <pid>]`.
pub fn cmd_queue(places: &Places, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: builds queue <repo> [top <pid>]";
    match args {
        [repo] => {
            check_repo(places, repo)?;
            let queue = Queue::of(places, repo);
            let order = queue.order();
            let holding = is_held(&places.root.join(repo).join("app.lock"))?;
            if let (true, Some(head)) = (holding, order.first().and_then(|name| pid_of(name))) {
                println!("holder {head} {}", command_of(head));
            }
            let waiting = queue.waiting();
            if waiting.is_empty() {
                println!("nobody waits");
            }
            for (at, pid) in waiting.iter().enumerate() {
                println!("{}. {pid} {}", at + 1, command_of(*pid));
            }
            Ok(())
        }
        [repo, word, pid] if word == "top" => match pid.parse() {
            Ok(pid) => top(places, repo, pid),
            Err(_) => fail!("{USAGE}"),
        },
        _ => fail!("{USAGE}"),
    }
}

/// `builds takeover <repo> [<pid>]`.
pub fn cmd_takeover(places: &Places, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: builds takeover <repo> [<pid>]";
    match args {
        [repo] => takeover(places, repo, None),
        [repo, pid] => match pid.parse() {
            Ok(pid) => takeover(places, repo, Some(pid)),
            Err(_) => fail!("{USAGE}"),
        },
        _ => fail!("{USAGE}"),
    }
}

fn command_of(pid: i32) -> String {
    procs::args(pid).map(|args| args.join(" ")).unwrap_or_else(|| "(gone)".to_string())
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
