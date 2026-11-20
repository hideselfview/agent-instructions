//! The line for a repo's app locks in a lane: <root>/<repo>/app.queue for the
//! mac lane, app-android.queue for the android lane (lane.rs). Each lane has
//! its own locks, one per app checkout, and one line for all of them.
//!
//! `order` names one ticket per line, next first. The holders' tickets stay in
//! it, and a ticket's name ends in where its build may run
//! (`<pid>-<n>-here|hosts|anywhere`, `lock::Where`). A waiter may take a lock
//! when a free checkout it may use is left once each waiter ahead of it has
//! taken one (`may_go`): a ticket whose pid is in the record beside a held
//! lock has taken its checkout already. A waiter that may use only a checkout
//! an earlier waiter cannot use goes ahead of that waiter; two that may use
//! the same checkouts go in the order they came.
//! Every change to it happens under a short guard lock, `.next`. A ticket is a
//! file named `<pid>-<n>-<where>` (n counts that process's tickets), held locked for
//! the life of its process and of every process that process starts, and
//! holding nothing else: the name carries the pid.
//!
//! A waiter blocks, in a thread inside `flock` for each, on the
//! tickets of every other entry (one of those leaving frees a lock or a place
//! for it). A build that takes a lock signals the waiters to read the locks
//! and its record again. Getting a ticket's lock
//! means the ticket's owner is gone, so the waiter takes the entry out of the
//! order and looks again: a waiter that dies in line never lets anyone past
//! the holders. SIGUSR1 means the order changed:
//! the waiter's own thread waits in a kqueue for that signal and for its
//! blocked threads, registered before it first reads the order, so a signal
//! is counted whether or not the waiter is blocked at that moment (a lost
//! signal would leave a waiter on the wrong ticket, and a reordering can turn
//! that into a cycle of waiters). The waiter then reads the order again and
//! starts a thread on the new predecessor; a thread left on the old one gives
//! any ticket it gets back at once, and ends when that ticket frees.
//!
//! `builds queue <repo>` shows each lane's checkouts, who holds each, why
//! builds added or removed one (lane_size.rs), and the lane's line;
//! `builds queue <repo> top <pid>`
//! moves a waiter to right behind the holder, and `builds takeover <repo>
//! [<pid>]` stops the holder's build and puts a waiter ahead of it (lock.rs);
//! a pid names its lane, and `--lane` names it when there is no pid.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::{AddedBy, Lane};
use crate::lock::Where;
use crate::lock_wake::{Shared, Wake};
use crate::places::Places;
use crate::procs::{self, Processes};
use crate::text::local_minute;

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

/// Opens <path> with <flags> and takes an exclusive flock on it, waiting for
/// the lock unless <flags> holds O_NONBLOCK. The descriptor is left open
/// without close-on-exec unless <flags> says so: a ticket or lock passes to
/// every process this one starts. A lock some process holds fails with
/// `WouldBlock` under O_NONBLOCK.
pub fn open_with(path: &Path, flags: libc::c_int) -> std::io::Result<libc::c_int> {
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("a path holds no NUL");
    let fd = loop {
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | flags, 0o644) };
        if fd >= 0 {
            break fd;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    let operation = if flags & libc::O_NONBLOCK != 0 {
        libc::LOCK_EX | libc::LOCK_NB
    } else {
        libc::LOCK_EX
    };
    loop {
        if unsafe { libc::flock(fd, operation) } == 0 {
            return Ok(fd);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            unsafe { libc::close(fd) };
            return Err(error);
        }
    }
}

pub fn open_locked(path: &Path) -> Result<libc::c_int> {
    open_with(path, libc::O_CREAT).context(|| format!("locking {}", path.display()))
}

/// The lock on the file at <path> (made if missing), or None when a process
/// holds it. Left open without close-on-exec, as `open_locked`.
pub fn try_lock(path: &Path) -> Result<Option<libc::c_int>> {
    match open_with(path, libc::O_CREAT | libc::O_NONBLOCK) {
        Ok(fd) => Ok(Some(fd)),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => fail!("locking {}: {error}", path.display()),
    }
}

/// Whether a process holds the lock on the file at <path>.
pub fn is_held(path: &Path) -> Result<bool> {
    // Close-on-exec: this descriptor only probes, and a process started while
    // it is open must not keep the lock.
    match open_with(path, libc::O_NONBLOCK | libc::O_CLOEXEC) {
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

/// Which checkouts the build behind <ticket> may take: the last word of its
/// name (`<pid>-<n>-<where>`).
pub fn allowed_of(ticket: &str) -> Result<Where> {
    match ticket.splitn(3, '-').nth(2).and_then(Where::parse) {
        Some(allowed) => Ok(allowed),
        None => fail!(
            "{ticket} is no ticket of this builds: its name does not end in where it may build"
        ),
    }
}

/// What the lane's locks say: its checkouts, those no build holds, and the
/// processes of the builds that hold the others.
struct Locks {
    entries: Vec<String>,
    free: Vec<String>,
    holders: Vec<i32>,
}

/// One ticket of this process: held locked until its descriptor closes.
pub struct Ticket {
    pub name: String,
    pub fd: libc::c_int,
}

/// <repo>'s queue folder for a lane, and the file naming the lane's app
/// checkouts, which says how many hold the lane's locks at once.
#[derive(Clone)]
pub struct Queue {
    dir: PathBuf,
    checkouts_file: PathBuf,
    /// The lane's locks, one file per app checkout.
    locks_dir: PathBuf,
}

/// The guard lock, held until dropped. It is not passed to processes this one
/// starts. Every change to the order, and every change to the lane's set of
/// app checkouts and every take of one of its locks, happens under it.
pub struct Guard(libc::c_int);

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// Takes the lock on <path>, creating the file and its folder when they are
/// missing, and waits until no other process holds it.
pub fn guard_at(path: &Path) -> Result<Guard> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    }
    open_with(path, libc::O_CREAT | libc::O_CLOEXEC)
        .map(Guard)
        .context(|| format!("locking {}", path.display()))
}

impl Queue {
    pub fn of(places: &Places, repo: &str, lane: Lane) -> Queue {
        Queue::in_repo_dir(&places.root.join(repo), lane)
    }

    pub fn in_repo_dir(repo_dir: &Path, lane: Lane) -> Queue {
        Queue {
            dir: repo_dir.join(lane.queue_dir_name()),
            checkouts_file: repo_dir.join(lane.checkouts_file_name()),
            locks_dir: repo_dir.join(lane.locks_dir_name()),
        }
    }

    fn lock_path(&self, checkout: &str) -> PathBuf {
        self.locks_dir.join(checkout)
    }

    /// The names of the lane's app checkouts, in the order recorded; none
    /// when the lane has no set yet.
    fn entries(&self) -> Result<Vec<String>> {
        let text = match std::fs::read_to_string(&self.checkouts_file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => fail!("reading {}: {error}", self.checkouts_file.display()),
        };
        Ok(crate::lane::parse_set(&text)?
            .into_iter()
            .map(|checkout| checkout.entry)
            .collect())
    }

    /// The lane's app checkouts that no build holds, and the pid in the
    /// record beside each lock a build holds: the processes whose tickets
    /// have taken a checkout already.
    fn locks(&self) -> Result<Locks> {
        let entries = self.entries()?;
        let (mut free, mut holders) = (Vec::new(), Vec::new());
        for entry in &entries {
            if is_held(&self.lock_path(entry))? {
                let record = crate::places::run_file_in(&self.locks_dir, entry);
                if let Some(run) = crate::holder_run::HolderRun::read(&record)? {
                    holders.push(run.pid);
                }
            } else {
                free.push(entry.clone());
            }
        }
        Ok(Locks {
            entries,
            free,
            holders,
        })
    }

    /// Where a waiter moved to the top goes: in front of the first ticket
    /// of <order> whose build holds no lock, so with a lock free it is the
    /// first that may take it.
    fn first_waiter(order: &[String], holders: &[i32]) -> usize {
        order
            .iter()
            .position(|name| !pid_of(name).is_some_and(|pid| holders.contains(&pid)))
            .unwrap_or(order.len())
    }

    /// How many builds the lane holds locks for at once: its app checkouts,
    /// read now, at least one.
    pub fn slots(&self) -> usize {
        let text = std::fs::read_to_string(&self.checkouts_file).unwrap_or_default();
        text.lines().filter(|line| !line.is_empty()).count().max(1)
    }

    fn ticket_path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn guard(&self) -> Result<Guard> {
        guard_at(&self.dir.join(".next"))
    }

    /// The tickets in line, next first. A write replaces the file whole, so a
    /// read without the guard sees one order or the other.
    pub fn order(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("order"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn write_order(&self, order: &[String]) -> Result<()> {
        let text: String = order.iter().map(|name| format!("{name}\n")).collect();
        crate::whole_file::write(&self.dir.join("order"), text)
    }

    /// The pid of each ticket in line whose build holds no lock, next first.
    pub fn waiting(&self) -> Result<Vec<i32>> {
        let holders = self.locks()?.holders;
        Ok(self
            .order()
            .iter()
            .filter_map(|name| pid_of(name))
            .filter(|pid| !holders.contains(pid))
            .collect())
    }

    /// Makes this process's next ticket, locked, and puts it last in line;
    /// returns it with how many were in line before it.
    pub fn join(&self, allowed: Where) -> Result<(Ticket, usize)> {
        let _guard = self.guard()?;
        let ticket = self.new_ticket(allowed)?;
        let mut order = self.order();
        let ahead = order.len();
        order.push(ticket.name.clone());
        self.write_order(&order)?;
        Ok((ticket, ahead))
    }

    /// A fresh ticket, locked, not yet in the order. Needs the guard.
    fn new_ticket(&self, allowed: Where) -> Result<Ticket> {
        let number = TICKETS.fetch_add(1, Ordering::SeqCst) + 1;
        let name = format!("{}-{number}-{}", std::process::id(), allowed.word());
        let fd = open_locked(&self.ticket_path(&name))?;
        Ok(Ticket { name, fd })
    }

    /// Gives <old> up for a new ticket that takes its place in line; <old>'s
    /// descriptor is the caller's to close.
    pub fn reissue(&self, old: &Ticket) -> Result<Ticket> {
        let _guard = self.guard()?;
        let new = self.new_ticket(allowed_of(&old.name)?)?;
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

    /// Takes <name> out of the order and removes its file: its owner is gone,
    /// or leaves the queue and will not join it again.
    pub fn drop_ticket(&self, name: &str) -> Result<()> {
        let _guard = self.guard()?;
        let mut order = self.order();
        if order.iter().any(|entry| entry == name) {
            order.retain(|entry| entry != name);
            self.write_order(&order)?;
        }
        self.unlink(name)
    }

    /// Whether the build behind <mine>, with the tickets <ahead> in front of
    /// it, may take a lock now: a free checkout it may use is left once each
    /// earlier waiter has taken one. A ticket whose owner holds a lock (its
    /// pid is in the record beside a held lock) has taken its checkout
    /// already, so only the other tickets ahead take one of the free ones.
    /// A ticket takes what its own where allows, and the tickets that may
    /// take any checkout are counted as taking the ones <mine> needs.
    fn may_go(&self, mine: &str, ahead: &[String]) -> Result<bool> {
        let allowed = allowed_of(mine)?;
        let Locks {
            entries,
            free,
            holders,
        } = self.locks()?;
        // With none <mine> may take, going on is what says so.
        if !entries.iter().any(|entry| allowed.allows(entry)) {
            return Ok(true);
        }
        let free_hosts = free
            .iter()
            .filter(|entry| Where::Hosts.allows(entry))
            .count();
        let free_here = free.len() - free_hosts;
        let (mut here, mut hosts, mut anywhere) = (0usize, 0usize, 0usize);
        for name in ahead {
            if pid_of(name).is_some_and(|pid| holders.contains(&pid)) {
                continue;
            }
            match allowed_of(name)? {
                Where::Here => here += 1,
                Where::Hosts => hosts += 1,
                Where::Anywhere => anywhere += 1,
            }
        }
        let left_here = free_here - here.min(free_here);
        let left_hosts = free_hosts - hosts.min(free_hosts);
        Ok(match allowed {
            Where::Here => left_here > anywhere,
            Where::Hosts => left_hosts > anywhere,
            Where::Anywhere => left_here + left_hosts > anywhere,
        })
    }

    /// Waits until a free checkout <mine> may use is left once the tickets
    /// ahead of it have taken theirs (`may_go`).
    pub fn wait_for_turn(&self, mine: &str) -> Result<()> {
        self.wait_for_turn_with(mine, || {})
    }

    /// `wait_for_turn`, calling <after_read> once the order is read and the
    /// waits on the tickets ahead started, before the process waits for an
    /// event: where a signal must not be lost.
    fn wait_for_turn_with(&self, mine: &str, mut after_read: impl FnMut()) -> Result<()> {
        route_repoint_signal();
        // Before the first read: a signal sent for a change this read
        // misses is counted from here on.
        let wake = Wake::new(libc::SIGUSR1)?;
        let shared = Arc::new(Mutex::new(Shared {
            current: std::collections::HashSet::new(),
            open: true,
        }));
        let mut generation = 0;
        // The tickets being waited on, each with the wait that started on it.
        let mut waiting: Vec<(u64, String)> = Vec::new();
        let result = (|| -> Result<()> {
            loop {
                let order = self.order();
                let Some(at) = order.iter().position(|name| name == mine) else {
                    fail!(
                        "{mine} is no longer in {}",
                        self.dir.join("order").display()
                    );
                };
                if self.may_go(mine, &order[..at])? {
                    return Ok(());
                }
                // Every other ticket: one leaving, a holder's or a waiter's
                // that died, changes what is left for this one.
                let ahead: Vec<&String> = order.iter().filter(|name| *name != mine).collect();
                waiting.retain(|(started, name)| {
                    let keep = ahead.contains(&name);
                    if !keep {
                        shared
                            .lock()
                            .expect("no panic holding it")
                            .current
                            .remove(started);
                    }
                    keep
                });
                for name in ahead {
                    if waiting.iter().all(|(_, waited)| waited != name) {
                        generation += 1;
                        shared
                            .lock()
                            .expect("no panic holding it")
                            .current
                            .insert(generation);
                        wake.block_on(self.ticket_path(name), generation, shared.clone())?;
                        waiting.push((generation, name.clone()));
                    }
                }
                after_read();
                if wake.wait()?.ready {
                    while let Some((from, code)) = wake.message() {
                        let Some(found) = waiting.iter().position(|(started, _)| *started == from)
                        else {
                            // A wait on a ticket since given up: it holds a
                            // ticket for this instant only.
                            if code >= 0 {
                                unsafe { libc::close(code) };
                            }
                            continue;
                        };
                        let (started, name) = waiting.remove(found);
                        shared
                            .lock()
                            .expect("no panic holding it")
                            .current
                            .remove(&started);
                        if code >= 0 {
                            let dropped = self.drop_ticket(&name);
                            unsafe { libc::close(code) };
                            dropped?;
                        } else if -code == libc::ENOENT {
                            self.drop_ticket(&name)?;
                        } else {
                            fail!(
                                "waiting on {}: {}",
                                self.ticket_path(&name).display(),
                                std::io::Error::from_raw_os_error(-code)
                            );
                        }
                    }
                }
            }
        })();
        // No thread of this wait reports from here on; what was reported and
        // not read is let go.
        wake.end(&shared);
        result
    }

    /// Moves the ticket of <pid> to right behind the builds that hold a
    /// lock.
    fn move_to_top(&self, pid: i32) -> Result<()> {
        let _guard = self.guard()?;
        let holders = self.locks()?.holders;
        let mut order = self.order();
        let Some(at) = order.iter().position(|name| pid_of(name) == Some(pid)) else {
            fail!("{pid} is not in the queue");
        };
        if holders.contains(&pid) {
            fail!("{pid} holds a lock");
        }
        let name = order.remove(at);
        order.insert(Queue::first_waiter(&order, &holders), name);
        self.write_order(&order)
    }
}

/// Whether <pid> is a `builds` process: a signal goes only to one, since the
/// pid in a ticket may be a stranger's by now.
fn is_builds(processes: &Processes, pid: i32) -> bool {
    processes
        .get(pid)
        .is_some_and(|process| process.name == "builds")
}

/// Sends <signal> to each of <pids> that is a `builds` process, not this one.
fn signal_all(pids: &[i32], signal: libc::c_int) -> Result<()> {
    let processes = Processes::read()?;
    for pid in pids {
        if *pid != std::process::id() as i32 && is_builds(&processes, *pid) {
            unsafe { libc::kill(*pid, signal) };
        }
    }
    Ok(())
}

fn pids_in(order: &[String]) -> Vec<i32> {
    order.iter().filter_map(|name| pid_of(name)).collect()
}

fn check_repo(places: &Places, repo: &str, lane: Lane) -> Result<()> {
    if repo.is_empty() || repo.starts_with('.') || repo.contains('/') {
        fail!("{repo:?} is not a repo's name");
    }
    if places.app_checkouts_of(repo, lane)?.is_empty() {
        fail!(
            "{repo} has no {} app checkout recorded; builds app-checkout records one",
            lane.word()
        );
    }
    Ok(())
}

/// The lanes of <repo> that have an app checkout recorded.
fn lanes_of(places: &Places, repo: &str) -> Result<Vec<Lane>> {
    if repo.is_empty() || repo.starts_with('.') || repo.contains('/') {
        fail!("{repo:?} is not a repo's name");
    }
    let mut lanes = Vec::new();
    for lane in Lane::ALL {
        if !places.app_checkouts_of(repo, lane)?.is_empty() {
            lanes.push(lane);
        }
    }
    if lanes.is_empty() {
        fail!("{repo} has no app checkout recorded; builds app-checkout records one");
    }
    Ok(lanes)
}

/// The lane whose line holds a ticket of <pid>.
fn lane_of_waiter(places: &Places, repo: &str, pid: i32) -> Result<Lane> {
    for lane in lanes_of(places, repo)? {
        if pids_in(&Queue::of(places, repo, lane).order()).contains(&pid) {
            return Ok(lane);
        }
    }
    fail!("{pid} is not in {repo}'s queue")
}

/// Moves the waiter <pid> to right behind the holder, in the lane it waits
/// in, and tells every ticket's process to read the order again.
pub fn top(places: &Places, repo: &str, pid: i32) -> Result<()> {
    let lane = lane_of_waiter(places, repo, pid)?;
    let queue = Queue::of(places, repo, lane);
    queue.move_to_top(pid)?;
    signal_all(&pids_in(&queue.order()), libc::SIGUSR1)
}

/// Puts the waiter <pid> (default the first of <lane>, or of the one lane
/// with waiters) ahead of the holder, stops the holder's build, and tells
/// every waiter to read the order again. The stopped build goes straight
/// behind the waiter and runs again by itself. With nothing holding the lock,
/// the same as `top`.
pub fn takeover(places: &Places, repo: &str, pid: Option<i32>, lane: Option<Lane>) -> Result<()> {
    let lane = match (pid, lane) {
        (Some(pid), None) => lane_of_waiter(places, repo, pid)?,
        (_, Some(lane)) => {
            check_repo(places, repo, lane)?;
            lane
        }
        (None, None) => {
            let waiting: Vec<Lane> = lanes_of(places, repo)?
                .into_iter()
                .filter(|lane| Queue::of(places, repo, *lane).order().len() > 1)
                .collect();
            match waiting[..] {
                [lane] => lane,
                [] => fail!("nobody waits in {repo}'s queues"),
                _ => fail!("builds wait in both of {repo}'s lanes; name one with --lane or a pid"),
            }
        }
    };
    let queue = Queue::of(places, repo, lane);
    let holder = {
        let _guard = queue.guard()?;
        let Locks { free, holders, .. } = queue.locks()?;
        let mut order = queue.order();
        let holds = |name: &String| pid_of(name).is_some_and(|pid| holders.contains(&pid));
        let taker = match pid {
            Some(pid) => order.iter().position(|name| pid_of(name) == Some(pid)),
            None => order.iter().position(|name| !holds(name)),
        };
        let Some(taker) = taker else {
            match pid {
                Some(pid) => fail!("{pid} is not in the queue"),
                None => fail!("nobody waits in {repo}'s {} queue", lane.word()),
            }
        };
        if holds(&order[taker]) {
            fail!("{} holds a lock", pid.unwrap_or_default());
        }
        let name = order.remove(taker);
        if !free.is_empty() {
            order.insert(Queue::first_waiter(&order, &holders), name);
            queue.write_order(&order)?;
            None
        } else {
            // Every lock is held: the build that joined last of the holders
            // is the one stopped.
            let Some(holder) = order.iter().rev().find(|name| holds(name)).cloned() else {
                fail!(
                    "every lock is held and no ticket in line is its holder's, so no build can be told to stop; kill its processes by pid"
                );
            };
            let processes = Processes::read()?;
            let Some(holder_pid) = pid_of(&holder).filter(|pid| is_builds(&processes, *pid)) else {
                fail!(
                    "the holder's `builds` is gone, so its build can't be told to stop; kill its processes by pid"
                );
            };
            order.insert(0, name);
            queue.write_order(&order)?;
            Some(holder_pid)
        }
    };
    let order = queue.order();
    let waiters: Vec<i32> = pids_in(&order)
        .into_iter()
        .filter(|pid| Some(*pid) != holder)
        .collect();
    signal_all(&waiters, libc::SIGUSR1)?;
    if let Some(holder) = holder {
        signal_all(&[holder], libc::SIGUSR2)?;
    }
    Ok(())
}

/// Tells every ticket's process in <lane>'s line to read the order again: its
/// set of app checkouts changed.
pub fn repoint(places: &Places, repo: &str, lane: Lane) -> Result<()> {
    signal_all(
        &pids_in(&Queue::of(places, repo, lane).order()),
        libc::SIGUSR1,
    )
}

/// Splits `--lane <lane>` out of <args>.
fn split_lane(args: &[String]) -> Result<(Vec<String>, Option<Lane>)> {
    let mut rest = Vec::new();
    let mut lane = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--lane" {
            let Some(word) = args.next() else {
                fail!("--lane takes mac or android")
            };
            lane = Some(Lane::parse_or_fail(word)?);
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((rest, lane))
}

/// `builds queue <repo> [--lane <lane>] [top <pid>]`.
pub fn cmd_queue(places: &Places, args: &[String]) -> Result<()> {
    const USAGE: &str =
        "usage: builds queue <repo> [--lane mac|android] | builds queue <repo> top <pid>";
    let (args, lane) = split_lane(args)?;
    match &args[..] {
        [repo] => {
            let lanes = match lane {
                Some(lane) => {
                    check_repo(places, repo, lane)?;
                    vec![lane]
                }
                None => lanes_of(places, repo)?,
            };
            for lane in lanes {
                let queue = Queue::of(places, repo, lane);
                let order = queue.order();
                let first: Vec<i32> = order.iter().filter_map(|name| pid_of(name)).collect();
                for recorded in places.app_checkouts_recorded(repo, lane)? {
                    let checkout = &recorded.entry;
                    let lock = places.app_lock_file(repo, lane, checkout);
                    if !is_held(&lock)? {
                        println!("{} {checkout}: free", lane.word());
                    } else {
                        let real = std::fs::canonicalize(&lock).unwrap_or(lock.clone());
                        let holder = first.iter().find(|pid| {
                            procs::open_files(**pid)
                                .iter()
                                .any(|open| *open == real || *open == lock)
                        });
                        match holder {
                            Some(pid) => println!(
                                "{} {checkout}: holder {pid} {}",
                                lane.word(),
                                command_of(*pid)
                            ),
                            None => println!("{} {checkout}: held", lane.word()),
                        }
                    }
                    if let AddedBy::Builds { at, why } = &recorded.added_by {
                        println!(
                            "{} {checkout}: builds added it at {}: {why}",
                            lane.word(),
                            local_minute(*at)
                        );
                    }
                }
                for removed in places.app_checkouts_removed(repo, lane)? {
                    println!(
                        "{}: builds removed {} at {}: {}",
                        lane.word(),
                        removed.entry,
                        local_minute(removed.at),
                        removed.why
                    );
                }
                let waiting = queue.waiting()?;
                if waiting.is_empty() {
                    println!("{}: nobody waits", lane.word());
                }
                for (at, pid) in waiting.iter().enumerate() {
                    println!("{}: {}. {pid} {}", lane.word(), at + 1, command_of(*pid));
                }
            }
            Ok(())
        }
        [repo, word, pid] if word == "top" && lane.is_none() => match pid.parse() {
            Ok(pid) => top(places, repo, pid),
            Err(_) => fail!("{USAGE}"),
        },
        _ => fail!("{USAGE}"),
    }
}

/// `builds takeover <repo> [--lane <lane>] [<pid>]`.
pub fn cmd_takeover(places: &Places, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: builds takeover <repo> [--lane mac|android] [<pid>]";
    let (args, lane) = split_lane(args)?;
    match &args[..] {
        [repo] => takeover(places, repo, None, lane),
        [repo, pid] => match pid.parse() {
            Ok(pid) => takeover(places, repo, Some(pid), lane),
            Err(_) => fail!("{USAGE}"),
        },
        _ => fail!("{USAGE}"),
    }
}

fn command_of(pid: i32) -> String {
    procs::args(pid)
        .map(|args| args.join(" "))
        .unwrap_or_else(|| "(gone)".to_string())
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
