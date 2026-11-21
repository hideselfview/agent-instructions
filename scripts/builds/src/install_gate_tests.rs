use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use super::*;
use crate::testing::Scratch;

fn holds(entered: Entered) -> Use {
    match entered {
        Entered::Holds(held) => held,
        Entered::RunAgain => panic!("the gate is open, so the command holds a use"),
    }
}

/// What a thread of the test tells the test.
#[derive(Debug, PartialEq)]
enum Event {
    /// The line the code under test gave its `say`.
    Said(String),
    /// What it returned: that the gates are closed, or that the command must
    /// run again or may go on.
    Done(bool),
}

/// How long a test waits for an event that must come before it fails: the
/// event, not this time, tells the test that something happened.
const PATIENCE: Duration = Duration::from_secs(30);

fn next(events: &Receiver<Event>) -> Event {
    events.recv_timeout(PATIENCE).expect("the event comes")
}

fn said(line: &str) -> Event {
    Event::Said(line.to_string())
}

/// Runs `close` on a thread, which sends each line `close` gives its `say`,
/// then `Done(true)` once it has closed both gates.
fn closing(places: &Places) -> (std::thread::JoinHandle<Closed>, Receiver<Event>) {
    let (events, received) = channel();
    let places = places.clone();
    let closer = std::thread::spawn(move || {
        let say = &mut |line| events.send(Event::Said(line)).unwrap();
        let closed = begin(&places, say).unwrap().close(say).unwrap();
        events.send(Event::Done(true)).unwrap();
        closed
    });
    (closer, received)
}

fn count(dir: &Path) -> usize {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries.count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("listing {}: {error}", dir.display()),
    }
}

#[test]
fn while_install_waits_for_a_command_use_connections_still_open_and_install_waits_for_them_next() {
    let scratch = Scratch::new("install-gate");
    let places = scratch.places();
    let quiet = &mut |_: &str| {};
    let hook_run = holds(enter_for_life(&places, 11, "checks run", "10:00:00", quiet).unwrap());
    let listing = PerConnection::new(places.clone(), 12, "item list", "10:00:01").unwrap();
    let (closer, install) = closing(&places);
    assert_eq!(
        next(&install),
        said("waiting for process 11, builds checks run, since 10:00:00"),
        "install closed the commands gate and waits for the hook run"
    );
    // A hook run that starts now waits at the closed gate.
    let (events, hook_run_2) = channel();
    let waiter = {
        let places = places.clone();
        std::thread::spawn(move || {
            let entered = enter_for_life(&places, 13, "checks run", "10:00:02", &mut |line| {
                events.send(Event::Said(line.to_string())).unwrap()
            })
            .unwrap();
            events
                .send(Event::Done(matches!(entered, Entered::RunAgain)))
                .unwrap();
        })
    };
    assert_eq!(next(&hook_run_2), said(RUNS_AGAIN));
    // A command that takes a use per connection starts, and its two
    // connections share one use.
    assert!(
        may_start(&places, &mut |line| panic!(
            "the connections gate is open: {line}"
        ))
        .unwrap()
    );
    let first = listing.take().unwrap();
    let second = listing.take().unwrap();
    assert_eq!(count(&Kind::Connection.uses(&places)), 1);
    drop(hook_run);
    assert_eq!(
        next(&install),
        said("waiting for process 12, builds item list, since 10:00:01"),
        "install closed the connections gate and waits for the open connections"
    );
    drop(first);
    assert!(
        holds_use(&places, Kind::Connection, 12),
        "one connection is open, so its use holds"
    );
    drop(second);
    assert_eq!(next(&install), Event::Done(true));
    let closed = closer.join().unwrap();
    // With both gates closed, a connection waits until install ends.
    let (events, opener_events) = channel();
    let opener = {
        let places = places.clone();
        std::thread::spawn(move || {
            let listing = PerConnection::new(places, 14, "item list", "10:00:03").unwrap();
            let taken = listing
                .take_saying(&mut |line| events.send(Event::Said(line.to_string())).unwrap())
                .map(drop);
            events.send(Event::Done(taken.is_ok())).unwrap();
        })
    };
    assert_eq!(
        next(&opener_events),
        said("builds install moves the stores: this command waits until it finishes")
    );
    drop(closed);
    assert_eq!(
        next(&hook_run_2),
        Event::Done(true),
        "the hook run runs again"
    );
    assert_eq!(next(&opener_events), Event::Done(true));
    waiter.join().unwrap();
    opener.join().unwrap();
    // Every use ended, so the next install waits for none and removes their
    // files.
    let mut lines = Vec::new();
    let say = &mut |line| lines.push(line);
    drop(begin(&places, say).unwrap().close(say).unwrap());
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(count(&Kind::Command.uses(&places)), 0);
    assert_eq!(count(&Kind::Connection.uses(&places)), 0);
}

/// Whether process <pid> holds its use of <kind>.
fn holds_use(places: &Places, kind: Kind, pid: u32) -> bool {
    let path = kind.uses(places).join(pid.to_string());
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(error) => panic!("opening {}: {error}", path.display()),
    };
    !try_lock(&file, libc::LOCK_EX, &path).unwrap()
}

/// A second install waits for the first to put its program on PATH, says
/// so, and begins once the first drops its lock, whether the first closed
/// the gates or not.
#[test]
fn a_second_install_waits_for_the_first() {
    let scratch = Scratch::new("install-gate-second");
    let places = scratch.places();
    let quiet = &mut |line: String| panic!("no install or use to wait for: {line}");
    let second = |places: &Places| {
        let (events, received) = channel();
        let places = places.clone();
        let install = std::thread::spawn(move || {
            let installing =
                begin(&places, &mut |line| events.send(Event::Said(line)).unwrap()).unwrap();
            events.send(Event::Done(true)).unwrap();
            installing
        });
        (install, received)
    };
    let first = begin(&places, quiet).unwrap();
    let (install, events) = second(&places);
    assert_eq!(
        next(&events),
        said("another builds install runs: waiting for it")
    );
    drop(first);
    assert_eq!(next(&events), Event::Done(true));
    let closed = install.join().unwrap().close(quiet).unwrap();
    let (install, events) = second(&places);
    assert_eq!(
        next(&events),
        said("another builds install runs: waiting for it")
    );
    drop(closed);
    assert_eq!(next(&events), Event::Done(true));
    drop(install.join().unwrap());
}

#[test]
fn the_file_of_a_use_whose_process_died_holds_no_install() {
    let scratch = Scratch::new("install-gate-dead");
    let places = scratch.places();
    for kind in [Kind::Command, Kind::Connection] {
        std::fs::create_dir_all(kind.uses(&places)).unwrap();
        std::fs::write(
            kind.uses(&places).join("99"),
            "process 99, builds item list, since 09:00:00\n",
        )
        .unwrap();
    }
    let mut said = Vec::new();
    let say = &mut |line| said.push(line);
    let closed = begin(&places, say).unwrap().close(say).unwrap();
    assert!(said.is_empty(), "{said:?}");
    assert_eq!(count(&Kind::Command.uses(&places)), 0);
    assert_eq!(count(&Kind::Connection.uses(&places)), 0);
    drop(closed);
}

/// A command that holds a use for a stretch of its life, as a landing does,
/// takes it at the gate, and sees an install that moved the stores since the
/// command started.
#[test]
fn a_held_use_sees_an_install_that_ran_before_it_took_it() {
    let scratch = Scratch::new("install-gate-held");
    let places = scratch.places();
    let landing = |pid: u32| PerConnection::new(places.clone(), pid, "land", "10:00:00").unwrap();
    let waiting = landing(21);
    let holding = landing(22);
    let held = holding.hold().unwrap();
    assert!(holds_use(&places, Kind::Command, 22));
    assert!(
        holding.take().unwrap().is_none(),
        "its connections take no use of their own"
    );
    assert!(!holding.install_ran().unwrap());
    drop(held);
    // An install runs before the other landing takes its use.
    drop(
        begin(&places, &mut |_| {})
            .unwrap()
            .close(&mut |_| {})
            .unwrap(),
    );
    let held = waiting.hold().unwrap();
    assert!(
        waiting.install_ran().unwrap(),
        "the landing runs the installed builds in its place"
    );
    drop(held);
    // The installed builds, run again, finds no install since it started.
    assert!(!landing(21).install_ran().unwrap());
}

/// An app build's lane holds no use while it builds, so an install that
/// moves the stores waits for no lane's steps; the lane then sees that the
/// install ran when it writes its run (handed_runs.rs).
#[test]
fn an_install_waits_for_no_app_build_lane() {
    let scratch = Scratch::new("install-gate-lane");
    let places = scratch.places();
    let building = PerConnection::new(
        places.clone(),
        23,
        "app-build demo --lane mac main",
        "10:00:00",
    )
    .unwrap();
    let mut said = Vec::new();
    let say = &mut |line| said.push(line);
    drop(begin(&places, say).unwrap().close(say).unwrap());
    assert!(said.is_empty(), "install waited for the lane: {said:?}");
    assert!(building.install_ran().unwrap());
}

/// A use removes its file when it ends, whether it lasts a command's life or
/// one connection.
#[test]
fn a_use_removes_its_file_when_it_ends() {
    let scratch = Scratch::new("install-gate-own-file");
    let places = scratch.places();
    let quiet = &mut |_: &str| {};
    let hook_run = holds(enter_for_life(&places, 31, "checks run", "10:00:00", quiet).unwrap());
    assert_eq!(count(&Kind::Command.uses(&places)), 1);
    drop(hook_run);
    assert_eq!(count(&Kind::Command.uses(&places)), 0);
    let listing = PerConnection::new(places.clone(), 32, "item list", "10:00:01").unwrap();
    let connection = listing.take().unwrap();
    assert_eq!(count(&Kind::Connection.uses(&places)), 1);
    drop(connection);
    assert_eq!(count(&Kind::Connection.uses(&places)), 0);
}

/// An install that waits for no use removes the file of each use whose
/// process died, and leaves the file of each use that is held.
#[test]
fn an_install_that_waits_for_no_use_removes_only_the_files_of_ended_uses() {
    let scratch = Scratch::new("install-gate-ended");
    let places = scratch.places();
    for kind in [Kind::Command, Kind::Connection] {
        std::fs::create_dir_all(kind.uses(&places)).unwrap();
        std::fs::write(
            kind.uses(&places).join("98"),
            "process 98, builds item list, since 09:00:00\n",
        )
        .unwrap();
    }
    let quiet = &mut |_: &str| {};
    let hook_run = holds(enter_for_life(&places, 33, "checks run", "10:00:00", quiet).unwrap());
    let listing = PerConnection::new(places.clone(), 34, "item list", "10:00:01").unwrap();
    let connection = listing.take().unwrap();
    begin(&places, &mut |line| {
        panic!("no install to wait for: {line}")
    })
    .unwrap()
    .remove_ended_uses()
    .unwrap();
    assert!(!Kind::Command.uses(&places).join("98").exists());
    assert!(!Kind::Connection.uses(&places).join("98").exists());
    assert!(holds_use(&places, Kind::Command, 33));
    assert!(holds_use(&places, Kind::Connection, 34));
    drop(connection);
    drop(hook_run);
}

/// A use file's path names the file only while no process removed it or
/// made another file there.
#[test]
fn a_path_names_its_file_until_the_file_is_removed_or_replaced() {
    let scratch = Scratch::new("install-gate-names");
    let path = scratch.dir.join("40");
    let file = open(&path).unwrap();
    assert!(names(&file, &path).unwrap());
    std::fs::remove_file(&path).unwrap();
    assert!(!names(&file, &path).unwrap());
    let again = open(&path).unwrap();
    assert!(!names(&file, &path).unwrap());
    assert!(names(&again, &path).unwrap());
}

/// `waiting` reads nothing while no install runs, then the gate a running
/// install closed and each use it waits for, in turn, without taking a use
/// or holding the install up.
#[test]
fn waiting_names_each_use_a_running_install_waits_for() {
    let scratch = Scratch::new("install-gate-waiting");
    let places = scratch.places();
    assert_eq!(waiting(&places).unwrap(), None, "no install ever ran");
    let quiet = &mut |_: &str| {};
    let hook_run = holds(enter_for_life(&places, 11, "checks run", "10:00:00", quiet).unwrap());
    let listing = PerConnection::new(places.clone(), 12, "item list", "10:00:01").unwrap();
    let connection = listing.take().unwrap();
    assert_eq!(waiting(&places).unwrap(), None, "no install runs");
    let (closer, install) = closing(&places);
    assert_eq!(
        next(&install),
        said("waiting for process 11, builds checks run, since 10:00:00")
    );
    assert_eq!(
        waiting(&places).unwrap(),
        Some(Waiting {
            connections_closed: false,
            held: vec![Held {
                pid: 11,
                text: "process 11, builds checks run, since 10:00:00".to_string(),
            }],
        })
    );
    drop(hook_run);
    assert_eq!(
        next(&install),
        said("waiting for process 12, builds item list, since 10:00:01")
    );
    assert_eq!(
        waiting(&places).unwrap(),
        Some(Waiting {
            connections_closed: true,
            held: vec![Held {
                pid: 12,
                text: "process 12, builds item list, since 10:00:01".to_string(),
            }],
        })
    );
    drop(connection);
    assert_eq!(next(&install), Event::Done(true));
    let closed = closer.join().unwrap();
    assert_eq!(
        waiting(&places).unwrap().map(|waiting| waiting.held),
        Some(Vec::new()),
        "install moves the stores with both gates closed and no use left"
    );
    drop(closed);
    // A child process that another test forks while the locks are held keeps
    // them until it calls exec, so wait for each lock to free before reading.
    drop(begin(&places, &mut |_| {}).unwrap());
    assert!(may_start(&places, quiet).unwrap());
    drop(holds(
        enter_for_life(&places, 15, "checks run", "10:00:04", quiet).unwrap(),
    ));
    assert_eq!(waiting(&places).unwrap(), None, "install ended");
}
