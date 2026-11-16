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
        let closed = close(&places, &mut |line| events.send(Event::Said(line)).unwrap()).unwrap();
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
    drop(close(&places, &mut |line| lines.push(line)).unwrap());
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
    !lock(&file, libc::LOCK_EX, false, &path).unwrap()
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
    let closed = close(&places, &mut |line| said.push(line)).unwrap();
    assert!(said.is_empty(), "{said:?}");
    assert_eq!(count(&Kind::Command.uses(&places)), 0);
    assert_eq!(count(&Kind::Connection.uses(&places)), 0);
    drop(closed);
}

#[test]
fn a_lane_holds_a_use_once_it_holds_its_lock_unless_an_install_ran_while_it_waited() {
    let scratch = Scratch::new("install-gate-lane");
    let places = scratch.places();
    let lane = |pid: u32| {
        PerConnection::new(
            places.clone(),
            pid,
            "app-build demo --lane mac main",
            "10:00:00",
        )
        .unwrap()
    };
    let waiting = lane(21);
    let building = lane(22);
    match building.hold_from_now().unwrap() {
        FromNow::Holds(held) => {
            assert!(
                building.take().unwrap().is_none(),
                "its connections take no use of their own"
            );
            drop(held);
        }
        _ => panic!("no install ran, so the lane holds its use"),
    }
    // An install runs while the other lane waits for its lock.
    drop(close(&places, &mut |_| {}).unwrap());
    assert!(matches!(
        waiting.hold_from_now().unwrap(),
        FromNow::InstallRan
    ));
    assert!(
        !holds_use(&places, Kind::Command, 21),
        "the lane that runs again holds no use"
    );
    // The installed builds, run again, holds its use.
    assert!(matches!(
        lane(21).hold_from_now().unwrap(),
        FromNow::Holds(_)
    ));
}
