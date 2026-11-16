//! `builds lock <repo> [--lane <lane>] <command> [<args>...]`: runs a command
//! holding one of the repo's app locks for a lane (the mac lane's unless
//! named): <root>/<repo>/app.locks/<checkout> for the mac lane, one per app
//! checkout of the lane (lane.rs). The lane's queue gives the free lock to the
//! next in line; the command gets the checkout whose lock it holds in
//! BUILDS_APP_CHECKOUT.
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
//! A command run with a time limit (`run_in_group_within`) is stopped when the
//! limit passes: its whole group gets SIGTERM, SIGKILL after `KILL_GRACE` if
//! the leader still runs, and SIGKILL once more for what the leader left
//! behind. It says `Within::TimedOut` and the caller fails the build, so the lock
//! goes to the next in line instead of staying with a build that hangs.
//!
//! Waiters go in the order of queue.rs. A takeover sends the holder SIGUSR2:
//! it stops its build's group and, once the build has exited, `run_in_group`
//! says `Ran::TakenOver`; the caller then takes a new place in the queue
//! (`Held::requeue`), straight behind the taker, and runs again.

use std::collections::HashMap;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::host::{self, Ssh};
use crate::lane::Lane;
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
    const USAGE: &str = "usage: builds lock <repo> [--lane mac|android] <command> [<args>...]";
    let (lane, args) = match args {
        [repo, flag, word, rest @ ..] if flag == "--lane" => (
            Lane::parse_or_fail(word)?,
            [std::slice::from_ref(repo), rest].concat(),
        ),
        _ => (Lane::Mac, args.to_vec()),
    };
    let [repo, command, rest @ ..] = &args[..] else {
        fail!("{USAGE}");
    };
    let mut held = take(places, repo, lane)?;
    loop {
        let mut start = Command::new(command);
        start.args(rest).env("BUILDS_APP_CHECKOUT", &held.checkout);
        match run_in_group(start, command)? {
            Ran::Finished => return Ok(()),
            Ran::TakenOver => {
                note(format!(
                    "{command} was stopped for a takeover; it runs again after it"
                ));
                held = held.requeue()?;
            }
        }
    }
}

/// Which of a lane's app checkouts a build may take, and where one step of an
/// app build may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// Only the side worktrees of this Mac: for a command that runs here
    /// (`builds lock`, `build-app`) and a step the repo keeps here (`here`
    /// lines).
    Here,
    /// Only the checkouts on hosts: for a test run that opens windows
    /// (`opens-windows` lines), which on this Mac would reach the person's
    /// screen.
    Hosts,
    /// Any of them (`app-build`, whose steps run where the checkout is).
    Anywhere,
}

impl Where {
    fn allows(self, entry: &str) -> bool {
        let on_host = crate::app_remote::split(entry).is_some();
        match self {
            Where::Here => !on_host,
            Where::Hosts => on_host,
            Where::Anywhere => true,
        }
    }

    /// The words that follow "runs only" in a message.
    pub fn only(self) -> &'static str {
        match self {
            Where::Here => "on this Mac",
            Where::Hosts => "on a host",
            Where::Anywhere => "anywhere",
        }
    }

    /// The words a message about the checkouts it allows puts after
    /// "checkout": nothing for `Anywhere`.
    fn checkouts(self) -> &'static str {
        match self {
            Where::Here => "on this Mac ",
            Where::Hosts => "on a host ",
            Where::Anywhere => "",
        }
    }
}

/// One of <repo>'s app locks for a lane, held for the life of this process and
/// every process it starts, unless released first.
pub struct Held {
    lock: libc::c_int,
    ticket: Ticket,
    queue: Queue,
    /// The app checkout (side-N) whose lock this is.
    pub checkout: String,
    places: Places,
    repo: String,
    lane: Lane,
    ssh: Ssh,
    allowed: Where,
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

    /// After a takeover, or a build the host's Xcode could not start: gives
    /// the lock and the place up, takes a new place where the old one stood,
    /// and waits for a lock again (the one just freed, or another's).
    pub fn requeue(self) -> Result<Held> {
        TAKEN_OVER.store(false, Ordering::SeqCst);
        let ticket = self.queue.reissue(&self.ticket)?;
        unsafe {
            libc::close(self.lock);
            libc::close(self.ticket.fd);
        }
        self.queue.wait_for_turn(&ticket.name)?;
        let (lock, checkout) =
            take_free_lock(&self.places, &self.ssh, &self.repo, self.lane, self.allowed)?;
        Ok(Held {
            lock,
            ticket,
            queue: self.queue,
            checkout,
            places: self.places,
            repo: self.repo,
            lane: self.lane,
            ssh: self.ssh,
            allowed: self.allowed,
        })
    }
}

/// The load average per core of each host, asked once for each when a build
/// first needs it.
struct Loads<'a> {
    places: &'a Places,
    ssh: &'a Ssh,
    asked: HashMap<String, std::result::Result<f64, String>>,
}

impl Loads<'_> {
    /// <host>'s load average per core, or what went wrong asking it.
    fn of(&mut self, host: &str) -> std::result::Result<f64, String> {
        if let Some(known) = self.asked.get(host) {
            return known.clone();
        }
        let probed = match host::all(self.places) {
            Err(error) => Err(error.text()),
            Ok(hosts) => match hosts.iter().find(|known| known.name == host) {
                None => Err(format!("`builds host list` does not have host {host}")),
                Some(known) => {
                    host::probe(self.places, self.ssh, known).map(|probe| probe.load_per_core())
                }
            },
        };
        self.asked.insert(host.to_string(), probed.clone());
        probed
    }
}

/// The host the app checkout <entry> is on.
fn host_of(entry: &str) -> &str {
    crate::app_remote::split(entry).map_or(host::LOCAL, |(host, _)| host)
}

/// <entries> in the order a build tries their locks: the checkouts on the
/// less loaded host first (load average per core; the order recorded breaks
/// a tie), with no asking when every entry is on this Mac. An entry whose host
/// does not answer is left out, and named in the second list.
fn by_load(loads: &mut Loads, entries: Vec<String>) -> (Vec<String>, Vec<String>) {
    if entries
        .iter()
        .all(|entry| crate::app_remote::split(entry).is_none())
    {
        return (entries, Vec::new());
    }
    let mut ranked = Vec::new();
    let mut skipped = Vec::new();
    for entry in entries {
        let host = host_of(&entry).to_string();
        match loads.of(&host) {
            Ok(load) => ranked.push((load, entry)),
            Err(error) => skipped.push(format!("{entry} (host {host}: {error})")),
        }
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    (
        ranked.into_iter().map(|(_, entry)| entry).collect(),
        skipped,
    )
}

/// Takes the first free lock among the lane's app checkouts <allowed> lets in,
/// the less loaded host's first and none on a host whose Xcode cannot start
/// when the lane builds with Xcode, waiting for one to free when the builds ahead
/// have not yet taken theirs or a stopped build is still going down; returns
/// it with the checkout's name. The lane's queue lets in as many as there are
/// locks, so a lock is free within moments.
fn take_free_lock(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    allowed: Where,
) -> Result<(libc::c_int, String)> {
    let dir = places.app_locks_dir(repo, lane);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let mut loads = Loads {
        places,
        ssh,
        asked: HashMap::new(),
    };
    loop {
        let checkouts: Vec<String> = places
            .app_checkouts_of(repo, lane)?
            .into_iter()
            .filter(|entry| allowed.allows(entry))
            .collect();
        if checkouts.is_empty() {
            fail!(
                "{repo} has no {} app checkout {}recorded; builds app-checkout records one",
                lane.word(),
                allowed.checkouts()
            );
        }
        let broken = if lane.uses_xcode() {
            host::refusing(places, host::Refused::MacBuilds)?
        } else {
            Vec::new()
        };
        let (on_broken, checkouts): (Vec<String>, Vec<String>) = checkouts
            .into_iter()
            .partition(|entry| broken.iter().any(|name| name == host_of(entry)));
        let (ordered, mut skipped) = by_load(&mut loads, checkouts);
        skipped.extend(on_broken.iter().map(|entry| {
            format!(
                "{entry} (host {}'s Xcode cannot start; `builds health` says what fixes it)",
                host_of(entry)
            )
        }));
        if ordered.is_empty() {
            fail!(
                "no app checkout of {repo}'s {} lane can take the build: {}",
                lane.word(),
                skipped.join("; ")
            );
        }
        for checkout in ordered {
            if let Some(lock) = queue::try_lock(&places.app_lock_file(repo, lane, &checkout))? {
                return Ok((lock, checkout));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// <checkout>'s own lock among <repo>'s app locks for <lane>, waiting until
/// no build holds it; never another checkout's. It skips the queue: a build
/// the queue let in for a lock takes another free one, or waits for one. The
/// descriptor holds the lock until closed, and every process started meanwhile
/// holds it too.
pub fn take_checkout(
    places: &Places,
    repo: &str,
    lane: Lane,
    checkout: &str,
) -> Result<libc::c_int> {
    let dir = places.app_locks_dir(repo, lane);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let file = places.app_lock_file(repo, lane, checkout);
    let mut waiting = false;
    loop {
        if let Some(lock) = queue::try_lock(&file)? {
            return Ok(lock);
        }
        if !waiting {
            note(format!(
                "waiting for the build holding {checkout}'s lock to end"
            ));
            waiting = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Joins <repo>'s queue for <lane> and waits its turn for one of the lane's app
/// locks on this Mac. Fails for a repo with no such app checkout in the lane.
pub fn take(places: &Places, repo: &str, lane: Lane) -> Result<Held> {
    take_in(places, &Ssh::from_env(), repo, lane, Where::Here)
}

/// `take` with <allowed>: with `Where::Anywhere`, the checkouts on hosts too,
/// the one on the less loaded host first; with `Where::Hosts`, only those.
pub fn take_where(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    allowed: Where,
) -> Result<Held> {
    take_in(places, ssh, repo, lane, allowed)
}

fn take_in(places: &Places, ssh: &Ssh, repo: &str, lane: Lane, allowed: Where) -> Result<Held> {
    if !places
        .app_checkouts_of(repo, lane)?
        .iter()
        .any(|entry| allowed.allows(entry))
    {
        fail!(
            "{repo} has no {} app checkout {}recorded in {}; builds app-checkout records one",
            lane.word(),
            allowed.checkouts(),
            places.app_checkouts_file(repo, lane).display()
        );
    }
    queue::route_repoint_signal();
    unsafe { libc::signal(libc::SIGUSR2, take_over as *const () as libc::sighandler_t) };
    TAKEN_OVER.store(false, Ordering::SeqCst);
    QUEUED.store(true, Ordering::SeqCst);
    let queue = Queue::of(places, repo, lane);
    let (ticket, ahead) = queue.join()?;
    if ahead >= queue.slots() {
        note(format!(
            "{ahead} ahead in {repo}'s {} app queue",
            lane.word()
        ));
    }
    queue.wait_for_turn(&ticket.name)?;
    let (lock, checkout) = take_free_lock(places, ssh, repo, lane, allowed)?;
    Ok(Held {
        lock,
        ticket,
        queue,
        checkout,
        places: places.clone(),
        repo: repo.to_string(),
        lane,
        ssh: ssh.clone(),
        allowed,
    })
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

/// How long a command that ignores SIGTERM gets, past its time limit, before
/// SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(10);

/// How a command run by `run_in_group` ended, when it didn't fail.
#[derive(Debug, PartialEq, Eq)]
pub enum Ran {
    Finished,
    /// A takeover stopped it: it didn't finish and is to run again.
    TakenOver,
}

/// How a command run by `run_in_group_within` ended, when it didn't fail.
#[derive(Debug, PartialEq, Eq)]
pub enum Within {
    Ran(Ran),
    /// It ran past its time limit and was killed.
    TimedOut,
}

/// Runs <start> in a process group of its own, passing SIGTERM, SIGINT and
/// SIGHUP on to the whole group, and fails with its exit status, unless a
/// takeover stopped it.
pub fn run_in_group(start: Command, command: &str) -> Result<Ran> {
    match run_in_group_within(start, command, None)? {
        Within::Ran(ran) => Ok(ran),
        Within::TimedOut => unreachable!("a command with no time limit cannot pass it"),
    }
}

/// `run_in_group`, killing the group once <limit> has passed (when there is
/// one) and saying `Within::TimedOut`.
pub fn run_in_group_within(
    start: Command,
    command: &str,
    limit: Option<Duration>,
) -> Result<Within> {
    run_group(start, command, limit, Keep::default())
}

/// What a command printed, kept as it arrived on both streams.
pub type Captured = Arc<Mutex<Vec<u8>>>;

/// The first line a command printed that holds the text a run watches for,
/// once one has come.
pub type Found = Arc<Mutex<Option<String>>>;

/// What a run keeps of the command's output, which it passes on to this
/// process's own output either way. With nothing to keep, the command writes
/// to this process's output itself.
#[derive(Default)]
pub struct Keep<'a> {
    /// All of it.
    pub all: Option<&'a Captured>,
    /// The first line that holds the text.
    pub line_with: Option<(&'static str, &'a Found)>,
}

impl Keep<'_> {
    fn anything(&self) -> bool {
        self.all.is_some() || self.line_with.is_some()
    }
}

/// How long to wait, after the command ends, for the end of its output: it
/// may leave a process behind that holds the pipe open.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// How much of a line with no end yet a watch holds. A line this long is not
/// one a watch looks for, so the watch keeps only its last `LINE_TAIL` bytes.
const LINE_LIMIT: usize = 64 * 1024;
const LINE_TAIL: usize = 1024;

/// One stream's watch for the first line that holds <needle>.
struct LineWatch {
    needle: &'static str,
    found: Found,
    /// The stream's line so far.
    line: Vec<u8>,
}

impl LineWatch {
    fn feed(&mut self, chunk: &[u8]) {
        for part in chunk.split_inclusive(|byte| *byte == b'\n') {
            self.line.extend_from_slice(part);
            if part.ends_with(b"\n") {
                self.end_line();
            } else if self.line.len() > LINE_LIMIT {
                self.line.drain(..self.line.len() - LINE_TAIL);
            }
        }
    }

    /// Looks at the line so far as a whole line and starts the next.
    fn end_line(&mut self) {
        let text = String::from_utf8_lossy(&self.line);
        if text.contains(self.needle) {
            let mut found = self.found.lock().expect("no thread panicked holding it");
            if found.is_none() {
                *found = Some(text.trim_end().to_string());
            }
        }
        self.line.clear();
    }
}

/// Copies <from> to <to>, keeping what <all> and <watch> ask for; sends on
/// <done> at the end.
fn copy_and_keep(
    mut from: impl std::io::Read,
    mut to: impl std::io::Write,
    all: Option<Captured>,
    mut watch: Option<LineWatch>,
    done: std::sync::mpsc::Sender<()>,
) {
    let mut chunk = [0u8; 8192];
    while let Ok(read) = from.read(&mut chunk) {
        if read == 0 {
            break;
        }
        let _ = to.write_all(&chunk[..read]);
        if let Some(all) = &all {
            all.lock()
                .expect("no thread panicked holding it")
                .extend_from_slice(&chunk[..read]);
        }
        if let Some(watch) = &mut watch {
            watch.feed(&chunk[..read]);
        }
    }
    if let Some(watch) = &mut watch {
        watch.end_line();
    }
    let _ = done.send(());
}

/// `run_in_group_within`, passing what the command prints on to this
/// process's own output as it arrives and keeping what <keep> asks for.
pub fn run_in_group_keeping(
    start: Command,
    command: &str,
    limit: Option<Duration>,
    keep: Keep,
) -> Result<Within> {
    run_group(start, command, limit, keep)
}

fn run_group(
    mut start: Command,
    command: &str,
    limit: Option<Duration>,
    keep: Keep,
) -> Result<Within> {
    if keep.anything() {
        start.stdout(std::process::Stdio::piped());
        start.stderr(std::process::Stdio::piped());
    }
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
    let (output_done, output_ended) = channel::<()>();
    let mut copies = 0;
    let all = || keep.all.cloned();
    let watch = || {
        keep.line_with.map(|(needle, found)| LineWatch {
            needle,
            found: found.clone(),
            line: Vec::new(),
        })
    };
    if let Some(stdout) = child.stdout.take() {
        let (all, watch, done) = (all(), watch(), output_done.clone());
        std::thread::spawn(move || copy_and_keep(stdout, std::io::stdout(), all, watch, done));
        copies += 1;
    }
    if let Some(stderr) = child.stderr.take() {
        let (all, watch, done) = (all(), watch(), output_done.clone());
        std::thread::spawn(move || copy_and_keep(stderr, std::io::stderr(), all, watch, done));
        copies += 1;
    }
    drop(output_done);
    // A takeover that came before the group was known stops it now.
    if TAKEN_OVER.load(Ordering::SeqCst) {
        unsafe { libc::kill(-group, libc::SIGTERM) };
    }
    // Dropping `ended` tells the watcher the command is over.
    let (ended, over) = channel::<()>();
    let timed_out = Arc::new(AtomicBool::new(false));
    let watcher = limit.map(|limit| {
        let timed_out = timed_out.clone();
        std::thread::spawn(move || {
            if over.recv_timeout(limit) != Err(RecvTimeoutError::Timeout) {
                return;
            }
            timed_out.store(true, Ordering::SeqCst);
            unsafe { libc::kill(-group, libc::SIGTERM) };
            if over.recv_timeout(KILL_GRACE) == Err(RecvTimeoutError::Timeout) {
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
        })
    });
    unsafe { libc::sigprocmask(libc::SIG_UNBLOCK, &passed, std::ptr::null_mut()) };
    let waited = child.wait();
    drop(ended);
    for _ in 0..copies {
        if output_ended.recv_timeout(OUTPUT_GRACE).is_err() {
            break;
        }
    }
    if let Some(watcher) = watcher {
        let _ = watcher.join();
    }
    if timed_out.load(Ordering::SeqCst) {
        // What the leader left behind. The group id is its pid, which stays
        // in use while any member of the group does.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    let status = waited.context(|| format!("waiting for {command}"))?;
    // The group is gone with its leader's pid free to be reused: no later
    // signal may go to it.
    GROUP.store(0, Ordering::SeqCst);
    match (status.code(), status.signal()) {
        _ if timed_out.load(Ordering::SeqCst) => Ok(Within::TimedOut),
        (Some(0), _) => Ok(Within::Ran(Ran::Finished)),
        _ if TAKEN_OVER.load(Ordering::SeqCst) => Ok(Within::Ran(Ran::TakenOver)),
        (Some(code), _) => Err(Fail::Exit(code)),
        (None, signal) => Err(Fail::Exit(128 + signal.unwrap_or(0))),
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
