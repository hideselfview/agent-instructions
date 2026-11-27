//! `builds lock <repo> [--lane <lane>] <command> [<args>...]`: runs a command
//! holding one of the repo's app locks for a lane (the mac lane's unless
//! named): <root>/<repo>/app.locks/<checkout> for the mac lane, one per app
//! checkout of the lane (lane.rs). The lane's queue gives the free lock to the
//! next in line; the command gets the checkout whose lock it holds in
//! BUILDS_APP_CHECKOUT.
//!
//! The build driver holds the checkout lock and queue ticket. An app build's
//! foreground commands get a guard that holds both until the command exits.
//! The command itself receives neither descriptor, even when it starts a
//! daemon that outlives it. `builds lock` passes the descriptors to the
//! external command to reserve the checkout for that command's process tree.
//!
//! The command runs in a process group of its own, and SIGTERM, SIGINT and
//! SIGHUP sent to this process are passed on to that whole group, so killing
//! the holder stops its build; the lock goes once the build has exited.
//!
//! Those stop signals end the holder's whole run, not only the command that
//! runs when one comes. The handler records the signal (`stop_signal`), and
//! every wait of a build ends on it: the wait for a turn in the queue
//! (queue.rs), for a free lock among the lane's checkouts and for a host's
//! window lock (window_lock.rs) each fail, naming the signal
//! (`refuse_when_stopped`), and a command started after the signal gets it
//! as soon as its group exists. `builds lock`, `builds build-app` and an app
//! build's lane each install the handler before they join the queue
//! (`end_on_stop_signals`), so a stop signal that comes between two steps, or
//! while the command waits, ends the command too.
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
//! the leader still runs, and SIGKILL once more, after the same grace, for
//! what the leader left behind. It says `Within::TimedOut` and the caller fails the build, so the lock
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
//! says `Ran::Stopped`; the caller then takes a new place in the queue
//! (`Held::requeue`), straight behind the taker, and runs again.

use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::app_lock_guard::CheckoutLock;
use crate::app_remote::host_of;
use crate::clock::Moment;
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
/// answers a stop.
static QUEUED: AtomicBool = AtomicBool::new(false);
/// A takeover or an install stopped, or is stopping, the build.
static STOPPED: AtomicBool = AtomicBool::new(false);
/// The stop signal this process got (`STOP_SIGNALS`); 0 before it got one.
static STOP_SIGNAL: AtomicI32 = AtomicI32::new(0);

/// The signals that end a build: whoever started it, a terminal or a
/// supervisor wants it to stop.
pub const STOP_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

/// Sends <signal> to the command's process group, when one runs.
extern "C" fn pass_on(signal: libc::c_int) {
    let group = GROUP.load(Ordering::SeqCst);
    if group > 0 {
        // kill is async-signal-safe.
        unsafe { libc::kill(-group, signal) };
    }
}

/// A stop signal: records it, then passes it on to the command's group.
extern "C" fn end_on(signal: libc::c_int) {
    STOP_SIGNAL.store(signal, Ordering::SeqCst);
    pass_on(signal);
}

/// Makes each of `STOP_SIGNALS` end this process's build: the handler records
/// the signal and passes it on to the group of the command that runs.
pub fn end_on_stop_signals() {
    unsafe {
        for signal in STOP_SIGNALS {
            libc::signal(signal, end_on as *const () as libc::sighandler_t);
        }
    }
}

/// The stop signal this process got since `end_on_stop_signals`, if any.
pub fn stop_signal() -> Option<libc::c_int> {
    match STOP_SIGNAL.load(Ordering::SeqCst) {
        0 => None,
        signal => Some(signal),
    }
}

/// The name of <signal>, one of `STOP_SIGNALS`: only the handlers of those
/// signals record one.
pub fn signal_name(signal: libc::c_int) -> &'static str {
    match signal {
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        libc::SIGHUP => "SIGHUP",
        other => unreachable!("signal {other} is not one of the stop signals"),
    }
}

/// How long a hold waits between its tries (`pause_before_trying_again`).
const TRY_AGAIN_AFTER: &str = "60";

/// Waits a minute between two tries of a held build, as a `sleep` in a group
/// of its own, so that a takeover ends the wait (`Ran::Stopped`) and a stop
/// signal fails the build, as for every command `run_in_group` runs.
pub fn pause_before_trying_again() -> Result<Ran> {
    let mut sleep = Command::new("sleep");
    sleep.arg(TRY_AGAIN_AFTER);
    run_in_group(sleep, "waiting to try again")
}

/// Fails, naming the stop signal and <waiting> (what the build was doing),
/// once this process got a stop signal.
pub fn refuse_when_stopped(waiting: &str) -> Result<()> {
    if let Some(signal) = stop_signal() {
        fail!(
            "the build stopped on {} while it {waiting}",
            signal_name(signal)
        );
    }
    Ok(())
}

/// SIGUSR2: stops the build, for a takeover or an install. Outside the time
/// between joining the queue and `Held::release` it does nothing.
extern "C" fn stop(_: libc::c_int) {
    if !QUEUED.load(Ordering::SeqCst) {
        return;
    }
    STOPPED.store(true, Ordering::SeqCst);
    pass_on(libc::SIGTERM);
}

/// Did a takeover or an install stop this build (SIGUSR2)?
pub fn takeover_stopped() -> bool {
    STOPPED.load(Ordering::SeqCst)
}

pub fn cmd_lock(places: &Places, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: builds lock <repo> --lane <app> <command> [<args>...]";
    let (lane, args) = match args {
        [repo, flag, word, rest @ ..] if flag == "--lane" => (
            crate::repo_lanes::parse(places, repo, word)?,
            [std::slice::from_ref(repo), rest].concat(),
        ),
        _ => fail!("{USAGE}"),
    };
    let [repo, command, rest @ ..] = &args[..] else {
        fail!("{USAGE}");
    };
    end_on_stop_signals();
    let mut held = take(places, repo, lane)?;
    loop {
        let mut start = Command::new(command);
        start.args(rest).env("BUILDS_APP_CHECKOUT", &held.checkout);
        held.checkout_lock().pass_to(&mut start);
        let ran = run_in_group(start, command);
        refuse_when_stopped(&format!("ran {command}"))?;
        match ran? {
            Ran::Finished => return Ok(()),
            // Only a takeover stops it: it holds no use of the stores, so
            // install never stops it.
            Ran::Stopped => {
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Where {
    /// Only the side worktrees of this Mac: for a command that runs here
    /// (`builds lock`, the android `build-app`) and a step the repo keeps here (`here`
    /// lines).
    Here,
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
            Where::Anywhere => "anywhere",
        }
    }

    /// The value <word> names, `word`'s inverse.
    pub fn parse(word: &str) -> Option<Where> {
        [Where::Here, Where::Anywhere]
            .into_iter()
            .find(|place| place.word() == word)
    }

    pub fn allows(self, entry: &str) -> bool {
        let on_host = crate::app_remote::split(entry).is_some();
        match self {
            Where::Here => !on_host,
            Where::Anywhere => true,
        }
    }

    /// The words that follow "checkout" in a message about which checkouts
    /// this value allows; empty for `Anywhere`.
    fn checkouts(self) -> &'static str {
        match self {
            Where::Here => "on this Mac ",
            Where::Anywhere => "",
        }
    }
}

/// Which checkouts a build takes: where it may run, and whether it draws
/// window scenes, so that it takes no host that cannot
/// (`host::screen_lacking`). This Mac always can.
#[derive(Clone, Copy)]
struct Takes {
    allowed: Where,
    screen: bool,
}

/// A build's reservation of an app checkout and its place in the lane's queue.
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
    pub fn checkout_lock(&self) -> CheckoutLock<'_> {
        // Held keeps the checkout descriptor open until it drops.
        unsafe { CheckoutLock::new(BorrowedFd::borrow_raw(self.lock), Some(self.ticket.as_fd())) }
    }

    /// Takes the window lock of this checkout's host for <step>, waiting
    /// while another run that opens windows holds it (window_lock.rs), and
    /// then, in the mac lane, while the host's screen is locked
    /// (host_screen_lock.rs). None when a takeover or an install stopped this
    /// build while it waited.
    pub fn take_window(
        &self,
        step: &str,
        access: crate::window_lock::Access,
    ) -> Result<Option<crate::window_lock::Window>> {
        let host_name = host_of(&self.checkout);
        let Some(window) = crate::window_lock::take_access(
            &self.places,
            host_name,
            &format!("{}'s {}, step {step}", self.repo, self.checkout),
            access,
            &|words| note(words),
            &|| STOPPED.load(Ordering::SeqCst),
        )?
        else {
            return Ok(None);
        };
        if !crate::app_platform::of(self.lane).opens_windows() {
            return Ok(Some(window));
        }
        let host = if host_name == host::LOCAL {
            host::Host::local()
        } else {
            host::remote_named(&self.places, &self.checkout, host_name)?
        };
        let unlocked = crate::host_screen_lock::wait_until_unlocked(
            host_name,
            &|| crate::host_screen_lock::read(&self.places, &self.ssh, &host),
            &|words| note(words),
            &|| crate::host_screen_lock::wait(&self.places, &self.ssh, &host),
        )?;
        Ok(unlocked.then_some(window))
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

    /// Records that this build runs no step: the record names none until
    /// `runs` names the next.
    pub fn runs_no_step(&self) -> Result<()> {
        self.record(None)
    }

    /// Records <step> as the step this build runs from now on.
    pub fn runs(&self, step: &str) -> Result<()> {
        self.record(Some(RunningStep {
            name: step.to_string(),
            started_at: crate::app_timing::now(),
        }))
    }

    /// Frees the lock and lets the next in the queue go.
    pub fn release(self) {
        // Past its build, a stop has nothing to stop.
        QUEUED.store(false, Ordering::SeqCst);
        drop(self);
    }

    /// After a takeover, or a build the host's Xcode could not start: gives
    /// the lock and the place up, takes a new place where the old one stood,
    /// and waits for a lock again (the one just freed, or another's).
    pub fn requeue(self) -> Result<Held> {
        STOPPED.store(false, Ordering::SeqCst);
        let waiting = Moment::now();
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
        let (lock, checkout) =
            take_free_lock(&places, &ssh, &repo, lane, takes, &queue, &ticket.name)?;
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
        unsafe { libc::close(self.lock) };
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
    /// <entries> that can draw a window scene, and the others, each with why
    /// not. An entry on this Mac can, and no question is asked for it.
    fn split(&mut self, entries: Vec<String>) -> (Vec<String>, Vec<String>) {
        let mut drawing = Vec::new();
        let mut undrawn = Vec::new();
        for entry in entries {
            if crate::app_remote::split(&entry).is_none() {
                drawing.push(entry);
                continue;
            }
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
    queue: &Queue,
    ticket: &str,
) -> Result<(libc::c_int, String)> {
    let Takes { allowed, screen } = takes;
    let dir = places.app_locks_dir(repo, lane);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let mut loads = Loads {
        places,
        ssh,
        asked: HashMap::new(),
    };
    // A build that draws takes no host that cannot (`take_drawing`).
    let mut screens = Screens {
        places,
        ssh,
        asked: HashMap::new(),
    };
    let mut waiting_for = Vec::new();
    loop {
        refuse_when_stopped(&format!(
            "waited for a free app checkout of {repo}'s {} lane",
            lane.word()
        ))?;
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
        let broken = if crate::app_platform::of(lane).builds_with_xcode() {
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
            let eligible: Vec<_> = ordered
                .into_iter()
                .filter(|entry| set.contains(entry))
                .collect();
            for checkout in &eligible {
                if let Some(lock) = queue::try_lock(&places.app_lock_file(repo, lane, checkout))? {
                    return Ok((lock, checkout.clone()));
                }
            }
            if eligible != waiting_for {
                queue.record_checkout_wait(ticket, &eligible)?;
                waiting_for = eligible;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// <checkout>'s own lock among <repo>'s app locks for <lane>, waiting until
/// no build holds it; never another checkout's. It skips the queue: a build
/// the queue let in for a lock takes another free one, or waits for one. The
/// descriptor holds the lock until closed and closes on exec.
pub fn take_checkout(places: &Places, repo: &str, lane: Lane, checkout: &str) -> Result<OwnedFd> {
    let dir = places.app_locks_dir(repo, lane);
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let file = places.app_lock_file(repo, lane, checkout);
    let mut waiting = false;
    loop {
        if let Some(lock) = queue::try_lock(&file)? {
            // try_lock returns a newly opened descriptor.
            return Ok(unsafe { OwnedFd::from_raw_fd(lock) });
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
/// the one on the less loaded host first.
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
/// shots line that passes `--windows`): the checkouts <allowed> lets in, less
/// the hosts that cannot draw them (`host::screen_lacking`), and it fails
/// naming each host it skipped when none can.
pub fn take_drawing(
    places: &Places,
    ssh: &Ssh,
    repo: &str,
    lane: Lane,
    allowed: Where,
    floor_kib: u64,
) -> Result<Held> {
    take_in(
        places,
        ssh,
        repo,
        lane,
        Takes {
            allowed,
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
    let has_one = places
        .app_checkouts_of(repo, lane)?
        .iter()
        .any(|entry| allowed.allows(entry));
    if !has_one {
        fail!(
            "{repo} has no {} app checkout {}recorded in {}; builds app-checkout records one",
            lane.word(),
            allowed.checkouts(),
            places.app_checkouts_file(repo, lane).display()
        );
    }
    queue::route_repoint_signal();
    if unsafe { libc::signal(libc::SIGUSR2, stop as *const () as libc::sighandler_t) }
        == libc::SIG_ERR
    {
        fail!(
            "installing the stop signal's handler: {}",
            std::io::Error::last_os_error()
        );
    }
    STOPPED.store(false, Ordering::SeqCst);
    QUEUED.store(true, Ordering::SeqCst);
    let queue = Queue::of(places, repo, lane);
    crate::lane_size::shrink(sized, crate::prune::Run::Real, now)?;
    let waiting = Moment::now();
    let (ticket, ahead) = queue.join(allowed)?;
    if ahead >= queue.slots() {
        note(format!(
            "{ahead} ahead in {repo}'s {} app queue",
            lane.word()
        ));
        if let Some(floor_kib) = grows {
            crate::lane_size::grow(sized, floor_kib, now)?;
        }
    }
    queue.wait_for_turn(&ticket.name)?;
    let (lock, checkout) = take_free_lock(places, ssh, repo, lane, takes, &queue, &ticket.name)?;
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

/// How long a command that ignores SIGTERM, and what its leader leaves behind
/// after SIGTERM, get past the time limit before SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(10);

/// How a command run by `run_in_group` ended, when it didn't fail.
#[derive(Debug, PartialEq, Eq)]
pub enum Ran {
    Finished,
    /// A takeover or an install stopped it: it didn't finish and is to run
    /// again.
    Stopped,
}

/// How a command run by `run_in_group_within` ended, when it didn't fail.
#[derive(Debug, PartialEq, Eq)]
pub enum Within {
    Ran(Ran),
    /// It ran past its time limit and was killed.
    TimedOut,
}

/// Runs <start> in a process group of its own, passing SIGTERM, SIGINT and
/// SIGHUP on to the whole group and recording them (`stop_signal`), and fails
/// with its exit status, unless a takeover or an install stopped it.
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
        for signal in STOP_SIGNALS {
            libc::sigaddset(&mut passed, signal);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &passed, std::ptr::null_mut());
    }
    end_on_stop_signals();
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
    // A stop that came before the group was known stops it now.
    if STOPPED.load(Ordering::SeqCst) {
        unsafe { libc::kill(-group, libc::SIGTERM) };
    }
    if let Some(signal) = stop_signal() {
        unsafe { libc::kill(-group, signal) };
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
        // What the leader left behind got the SIGTERM too: a git among it
        // removes its lock files on SIGTERM, and SIGKILL before it has done
        // so leaves an index.lock that fails every later build in the
        // checkout. So the rest of the group gets `KILL_GRACE` to end, then
        // SIGKILL. The group id is the leader's pid, which stays in use while
        // any member of the group does.
        let until = Instant::now() + KILL_GRACE;
        while unsafe { libc::kill(-group, 0) } == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    let status = waited.context(|| format!("waiting for {command}"))?;
    // The group is gone with its leader's pid free to be reused: no later
    // signal may go to it.
    GROUP.store(0, Ordering::SeqCst);
    match crate::fail::exit_code(status) {
        _ if timed_out.load(Ordering::SeqCst) => Ok(Within::TimedOut),
        0 => Ok(Within::Ran(Ran::Finished)),
        _ if STOPPED.load(Ordering::SeqCst) => Ok(Within::Ran(Ran::Stopped)),
        code => Err(Fail::Exit(code)),
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
