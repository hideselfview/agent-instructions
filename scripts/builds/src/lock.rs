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
//! The holder keeps a record of its run beside the lock (holder_run.rs): when
//! it joined the queue, how long it waited, and the step it runs now
//! (`Held::runs`). When it lets the lock go it writes the time beside the lock
//! too (`<checkout>.ended`), so lane_size.rs knows how long the checkout has
//! had no build.
//!
//! A build reads the lane's set of app checkouts and takes a free lock among
//! them under the queue's guard (queue.rs `Queue::guard`), and every change to
//! the set happens under that guard too, so no build takes the lock of a
//! checkout that a change has just taken out of the set. A build that joins
//! the queue first lets lane_size.rs remove the checkouts builds added that no
//! build used for a while, and a build that has to wait lets it add one on a
//! host.
//!
//! Waiters go in the order of queue.rs. A takeover sends the holder SIGUSR2:
//! it stops its build's group and, once the build has exited, `run_in_group`
//! says `Ran::TakenOver`; the caller then takes a new place in the queue
//! (`Held::requeue`), straight behind the taker, and runs again.

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::holder_run::{HolderRun, RunningStep};
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
    /// (`builds lock`, the android `build-app`) and a step the repo keeps here (`here`
    /// lines).
    Here,
    /// Only the checkouts on hosts: for a test run that opens windows
    /// (`opens-windows` lines), which on this Mac would reach the person's
    /// screen.
    Hosts,
    /// Any of them (`app-build`, whose steps run where the checkout is, and
    /// the mac `build-app`, which copies its app back).
    Anywhere,
}

impl Where {
    /// The word a ticket's name carries (queue.rs): which checkouts the
    /// build behind the ticket may take.
    pub fn word(self) -> &'static str {
        match self {
            Where::Here => "here",
            Where::Hosts => "hosts",
            Where::Anywhere => "anywhere",
        }
    }

    /// The value <word> names, `word`'s inverse.
    pub fn parse(word: &str) -> Option<Where> {
        [Where::Here, Where::Hosts, Where::Anywhere]
            .into_iter()
            .find(|place| place.word() == word)
    }

    pub fn allows(self, entry: &str) -> bool {
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

    /// The words that follow "checkout" in a message about which checkouts
    /// this value allows; empty for `Anywhere`.
    fn checkouts(self) -> &'static str {
        match self {
            Where::Here => "on this Mac ",
            Where::Hosts => "on a host ",
            Where::Anywhere => "",
        }
    }
}

/// Which checkouts a build takes: where it may run, and whether it draws
/// window scenes, so that it takes no host that cannot
/// (`host::screen_lacking`).
#[derive(Clone, Copy)]
struct Takes {
    allowed: Where,
    screen: bool,
}

/// One of <repo>'s app locks for a lane, held until its owner drops this value
/// and every process this one started meanwhile has exited.
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
    takes: Takes,
    /// When this build joined the lane's queue, seconds since 1970.
    pub joined_at: i64,
    /// How long it waited in the queue in all, a wait after a takeover
    /// included.
    pub waited: Duration,
}

impl Held {
    /// Takes the window lock of this checkout's host for <step>, waiting
    /// while another run that opens windows holds it (window_lock.rs). None
    /// when a takeover stopped this build while it waited.
    pub fn take_window(&self, step: &str) -> Result<Option<crate::window_lock::Window>> {
        crate::window_lock::take(
            &self.places,
            host_of(&self.checkout),
            &format!("{}'s {}, step {step}", self.repo, self.checkout),
            &|words| note(words),
            &|| TAKEN_OVER.load(Ordering::SeqCst),
        )
    }

    /// Writes the record of this build's run beside its lock, naming <step>
    /// as the step it runs now.
    fn record(&self, step: Option<RunningStep>) -> Result<()> {
        HolderRun {
            pid: std::process::id() as i32,
            joined_at: self.joined_at,
            waited_ms: self.waited.as_millis() as u64,
            step,
        }
        .write(
            &self
                .places
                .app_lock_run_file(&self.repo, self.lane, &self.checkout),
        )
    }

    /// Records <step> as the step this build runs from now on.
    pub fn runs(&self, step: &str) -> Result<()> {
        self.record(Some(RunningStep {
            name: step.to_string(),
            started_at: crate::app_timing::now(),
        }))
    }

    /// Frees the lock and lets the next in the queue go. No process this one
    /// started may still run.
    pub fn release(self) {
        // Past its build, a takeover has nothing to stop.
        QUEUED.store(false, Ordering::SeqCst);
        drop(self);
    }

    /// Takes this build out of the queue, then frees the lock, which lets the
    /// next in the queue go: for a build that will not wait for the lock
    /// again, such as a build that runs the installed builds instead of
    /// building. No process this one started may still run.
    pub fn leave(self) -> Result<()> {
        let left = self.queue.drop_ticket(&self.ticket.name);
        self.release();
        left
    }

    /// After a takeover, or a build the host's Xcode could not start: gives
    /// the lock and the place up, takes a new place where the old one stood,
    /// and waits for a lock again (the one just freed, or another's).
    pub fn requeue(self) -> Result<Held> {
        TAKEN_OVER.store(false, Ordering::SeqCst);
        let waiting = Instant::now();
        let ticket = self.queue.reissue(&self.ticket)?;
        let queue = self.queue.clone();
        let places = self.places.clone();
        let repo = self.repo.clone();
        let ssh = self.ssh.clone();
        let (lane, takes, joined_at, waited) = (self.lane, self.takes, self.joined_at, self.waited);
        // Records the end of this use of the checkout and frees its lock and
        // the old ticket.
        drop(self);
        queue.wait_for_turn(&ticket.name)?;
        let (lock, checkout) = take_free_lock(&places, &ssh, &repo, lane, takes)?;
        let held = Held {
            lock,
            ticket,
            queue,
            checkout,
            places,
            repo,
            lane,
            ssh,
            takes,
            joined_at,
            waited: waited + waiting.elapsed(),
        };
        held.record(None)?;
        queue::repoint(&held.places, &held.repo, held.lane)?;
        Ok(held)
    }
}

impl Drop for Held {
    /// Writes when the build let the checkout go, while it still holds the
    /// lock so that the next build's use of the checkout never gets this
    /// time, then frees the lock and the build's place in the queue.
    fn drop(&mut self) {
        let ended = self
            .places
            .app_lock_ended_file(&self.repo, self.lane, &self.checkout);
        if let Err(error) = crate::whole_file::write(&ended, crate::app_timing::now().to_string()) {
            note(format!(
                "could not record when this build let {} go: {}",
                self.checkout,
                error.text()
            ));
        }
        unsafe {
            libc::close(self.lock);
            libc::close(self.ticket.fd);
        }
    }
}

/// What each host answered to one question, asked once for each when a build
/// first needs it: its load average per core (`Loads`), or whether it can
/// draw a window scene (`Screens`).
struct Asked<'a, T> {
    places: &'a Places,
    ssh: &'a Ssh,
    asked: HashMap<String, std::result::Result<T, String>>,
}

type Loads<'a> = Asked<'a, f64>;
type Screens<'a> = Asked<'a, Option<String>>;

impl<T: Clone> Asked<'_, T> {
    /// <host>'s answer to <ask>, or what went wrong asking it.
    fn ask(
        &mut self,
        host: &str,
        ask: impl Fn(&Ssh, &host::Host) -> std::result::Result<T, String>,
    ) -> std::result::Result<T, String> {
        if let Some(known) = self.asked.get(host) {
            return known.clone();
        }
        let answer = match host::all(self.places) {
            Err(error) => Err(error.text()),
            Ok(hosts) => match hosts.iter().find(|known| known.name == host) {
                None => Err(format!("`builds host list` does not have host {host}")),
                Some(known) => ask(self.ssh, known),
            },
        };
        self.asked.insert(host.to_string(), answer.clone());
        answer
    }
}

impl Loads<'_> {
    /// <host>'s load average per core, or what went wrong asking it.
    fn of(&mut self, host: &str) -> std::result::Result<f64, String> {
        let places = self.places;
        self.ask(host, |ssh, known| {
            host::probe(places, ssh, known).map(|probe| probe.load_per_core())
        })
    }
}

impl Screens<'_> {
    /// <entries>, each on a host, that can draw a window scene, and the
    /// others, each with why not.
    fn split(&mut self, entries: Vec<String>) -> (Vec<String>, Vec<String>) {
        let mut drawing = Vec::new();
        let mut undrawn = Vec::new();
        for entry in entries {
            let host = host_of(&entry).to_string();
            match self.ask(&host, host::screen_lacks) {
                Ok(None) => drawing.push(entry),
                Ok(Some(why)) => undrawn.push(format!(
                    "{entry} (on host {host}, {why}, so it cannot draw shots; open its lid or attach a display)"
                )),
                Err(said) => undrawn.push(format!("{entry} ({said})")),
            }
        }
        (drawing, undrawn)
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
/// the less loaded host's first and none on a host recorded as refusing mac
/// app builds (`host::refusing_mac_builds`) when the lane builds with Xcode,
/// waiting for one to free when the builds ahead
/// have not yet taken theirs or a stopped build is still going down; returns
/// it with the checkout's name. The lane's queue lets in as many as there are
/// locks, so a lock is free within moments. The build asks the hosts for
/// their load outside the queue's guard, then reads the set again and takes
/// the lock under the guard.
fn take_free_lock(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    takes: Takes,
) -> Result<(libc::c_int, String)> {
    let Takes { allowed, screen } = takes;
    let dir = places.app_locks_dir(repo, lane);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let queue = Queue::of(places, repo, lane);
    let mut loads = Loads {
        places,
        ssh,
        asked: HashMap::new(),
    };
    // A build that draws takes only checkouts on hosts (`take_drawing`).
    let mut screens = Screens {
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
            host::refusing_mac_builds(places)?
        } else {
            Vec::new()
        };
        let cause_on = |entry: &str| {
            broken
                .iter()
                .find(|refusal| refusal.host == host_of(entry))
                .and_then(|refusal| refusal.refused.mac_builds_cause())
        };
        let (on_broken, checkouts): (Vec<String>, Vec<String>) = checkouts
            .into_iter()
            .partition(|entry| cause_on(entry).is_some());
        // A build that draws window scenes takes no host that cannot.
        let (checkouts, undrawn) = if screen {
            screens.split(checkouts)
        } else {
            (checkouts, Vec::new())
        };
        let (ordered, mut skipped) = by_load(&mut loads, checkouts);
        skipped.extend(undrawn);
        skipped.extend(on_broken.iter().map(|entry| {
            format!(
                "{entry} (on host {}, {}; `builds health` says what fixes it)",
                host_of(entry),
                cause_on(entry).expect("the partition put only an entry with a cause here")
            )
        }));
        if ordered.is_empty() {
            fail!(
                "no app checkout of {repo}'s {} lane can take the build: {}",
                lane.word(),
                skipped.join("; ")
            );
        }
        {
            let _guard = queue.guard()?;
            let set = places.app_checkouts_of(repo, lane)?;
            for checkout in ordered.into_iter().filter(|entry| set.contains(entry)) {
                if let Some(lock) = queue::try_lock(&places.app_lock_file(repo, lane, &checkout))? {
                    return Ok((lock, checkout));
                }
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
    take_in(
        places,
        &Ssh::from_env(),
        repo,
        lane,
        Takes {
            allowed: Where::Here,
            screen: false,
        },
        None,
        crate::app_timing::now(),
    )
}

/// `take` with <allowed>: with `Where::Anywhere`, the checkouts on hosts too,
/// the one on the less loaded host first; with `Where::Hosts`, only those.
/// A build that may take a checkout on a host and has to wait adds one on a
/// host whose free disk stays above <floor_kib> (lane_size.rs).
pub fn take_where(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    allowed: Where,
    floor_kib: u64,
) -> Result<Held> {
    let grows = (allowed != Where::Here).then_some(floor_kib);
    take_in(
        places,
        ssh,
        repo,
        lane,
        Takes {
            allowed,
            screen: false,
        },
        grows,
        crate::app_timing::now(),
    )
}

/// `take_where` for a build that draws window scenes (a `--shots` run of a
/// shots line that passes `--windows`): only the checkouts on hosts that can
/// draw them (`host::screen_lacking`), and it fails naming each host it
/// skipped when none can.
pub fn take_drawing(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    floor_kib: u64,
) -> Result<Held> {
    take_in(
        places,
        ssh,
        repo,
        lane,
        Takes {
            allowed: Where::Hosts,
            screen: true,
        },
        Some(floor_kib),
        crate::app_timing::now(),
    )
}

/// Joins the queue and waits for a lock. <grows> is the free-disk floor of
/// the hosts when the build may add a checkout on one; None when it never
/// does. <now> is the time the build starts, in seconds since 1970.
fn take_in(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    takes: Takes,
    grows: Option<u64>,
    now: i64,
) -> Result<Held> {
    let allowed = takes.allowed;
    let sized = crate::lane_size::RepoLane {
        places,
        ssh,
        repo,
        lane,
    };
    let has_one = |places: &Places| -> Result<bool> {
        Ok(places
            .app_checkouts_of(repo, lane)?
            .iter()
            .any(|entry| allowed.allows(entry)))
    };
    let mut usable = has_one(places)?;
    if !usable
        && allowed == Where::Hosts
        && let Some(floor_kib) = grows
        && !places.app_checkouts_of(repo, lane)?.is_empty()
    {
        crate::lane_size::grow(sized, floor_kib, crate::lane_size::Need::OnAHost, now)?;
        usable = has_one(places)?;
    }
    if !usable {
        fail!(
            "{repo} has no {} app checkout {}recorded in {}; builds app-checkout records one",
            lane.word(),
            allowed.checkouts(),
            places.app_checkouts_file(repo, lane).display()
        );
    }
    queue::route_repoint_signal();
    if unsafe { libc::signal(libc::SIGUSR2, take_over as *const () as libc::sighandler_t) }
        == libc::SIG_ERR
    {
        fail!(
            "installing the takeover signal's handler: {}",
            std::io::Error::last_os_error()
        );
    }
    TAKEN_OVER.store(false, Ordering::SeqCst);
    QUEUED.store(true, Ordering::SeqCst);
    let queue = Queue::of(places, repo, lane);
    crate::lane_size::shrink(sized, crate::prune::Run::Real, now)?;
    let waiting = Instant::now();
    let (ticket, ahead) = queue.join(allowed)?;
    if ahead >= queue.slots() {
        note(format!(
            "{ahead} ahead in {repo}'s {} app queue",
            lane.word()
        ));
        if let Some(floor_kib) = grows {
            crate::lane_size::grow(sized, floor_kib, crate::lane_size::Need::Waits, now)?;
        }
    }
    queue.wait_for_turn(&ticket.name)?;
    let (lock, checkout) = take_free_lock(places, ssh, repo, lane, takes)?;
    let held = Held {
        lock,
        ticket,
        queue,
        checkout,
        places: places.clone(),
        repo: repo.to_string(),
        lane,
        ssh: ssh.clone(),
        takes,
        joined_at: now,
        waited: waiting.elapsed(),
    };
    held.record(None)?;
    // The waiters behind read the locks and the holders' records again: a
    // waiter that could not tell this ticket from a waiter waits for this.
    queue::repoint(places, repo, lane)?;
    Ok(held)
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

/// The first line a command printed that holds one of the texts a run
/// watches for, once one has come.
pub type Found = Arc<Mutex<Option<String>>>;

/// A reader that a run gives each whole line of the command's output, from
/// either stream, without its newline.
pub type EachLine = Arc<dyn Fn(&str) + Send + Sync>;

/// What a run keeps of the command's output, which it passes on to this
/// process's own output either way. With nothing to keep, the command writes
/// to this process's output itself.
#[derive(Default)]
pub struct Keep<'a> {
    /// All of it.
    pub all: Option<&'a Captured>,
    /// The first line that holds one of the texts.
    pub line_with: Option<(&'static [&'static str], &'a Found)>,
    /// Each line, given to the reader.
    pub each_line: Option<&'a EachLine>,
}

impl Keep<'_> {
    fn anything(&self) -> bool {
        self.all.is_some() || self.line_with.is_some() || self.each_line.is_some()
    }

    /// What reads one stream's lines; None when nothing reads lines.
    fn lines(&self) -> Option<StreamLines> {
        (self.line_with.is_some() || self.each_line.is_some()).then(|| StreamLines {
            watch: self
                .line_with
                .map(|(needles, found)| (needles, found.clone())),
            each: self.each_line.cloned(),
            line: Vec::new(),
        })
    }
}

/// How long to wait, after the command ends, for the end of its output: it
/// may leave a process behind that holds the pipe open.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// How much of a line with no end yet a watch holds. A line this long is not
/// one a watch looks for, so the watch keeps only its last `LINE_TAIL` bytes.
const LINE_LIMIT: usize = 64 * 1024;
const LINE_TAIL: usize = 1024;

/// One stream's lines: a watch for the first line that holds one of its
/// texts, and a reader of each line.
struct StreamLines {
    watch: Option<(&'static [&'static str], Found)>,
    each: Option<EachLine>,
    /// The stream's line so far.
    line: Vec<u8>,
}

impl StreamLines {
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

    /// Looks at the line so far as a whole line and starts the next. A
    /// stream that ended on a newline has no line left to look at.
    fn end_line(&mut self) {
        let text = String::from_utf8_lossy(&self.line);
        if let Some((needles, found)) = &self.watch
            && needles.iter().any(|needle| text.contains(needle))
        {
            let mut found = found.lock().expect("no thread panicked holding it");
            if found.is_none() {
                *found = Some(text.trim_end().to_string());
            }
        }
        if let Some(each) = &self.each {
            each(text.trim_end_matches(['\n', '\r']));
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
    mut watch: Option<StreamLines>,
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
    if let Some(watch) = &mut watch
        && !watch.line.is_empty()
    {
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
    let watch = || keep.lines();
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
    match crate::fail::exit_code(status) {
        _ if timed_out.load(Ordering::SeqCst) => Ok(Within::TimedOut),
        0 => Ok(Within::Ran(Ran::Finished)),
        _ if TAKEN_OVER.load(Ordering::SeqCst) => Ok(Within::Ran(Ran::TakenOver)),
        code => Err(Fail::Exit(code)),
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
