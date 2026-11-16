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

/// Starts an install's close on a thread: what it said arrives on the
/// returned channel once both gates are closed.
fn closing(places: &Places) -> (std::thread::JoinHandle<Closed>, Receiver<Vec<String>>) {
    let (said_tx, said_rx) = channel();
    let places = places.clone();
    let closer = std::thread::spawn(move || {
        let mut said = Vec::new();
        let closed = close(&places, &mut |line| said.push(line)).unwrap();
        said_tx.send(said).unwrap();
        closed
    });
    (closer, said_rx)
}

fn count(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| entries.count())
}

#[test]
fn while_install_waits_for_a_command_use_connections_still_open_and_install_waits_for_them_next() {
    let scratch = Scratch::new("install-gate");
    let places = scratch.places();
    let hook_run = holds(enter_for_life(&places, 11, "checks run", "10:00:00").unwrap());
    let listing = PerConnection::new(places.clone(), 12, "item list", "10:00:01");
    let (closer, said) = closing(&places);
    assert!(
        said.recv_timeout(Duration::from_millis(300)).is_err(),
        "install waits while the hook run runs"
    );
    // The commands gate is closed: a hook run that starts now waits.
    let (ran_tx, ran_rx) = channel();
    let waiter = {
        let places = places.clone();
        std::thread::spawn(move || {
            let entered = enter_for_life(&places, 13, "checks run", "10:00:02").unwrap();
            ran_tx.send(matches!(entered, Entered::RunAgain)).unwrap();
        })
    };
    assert!(
        ran_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "the hook run waits at the closed gate"
    );
    // A command that takes a use per connection starts, and its two
    // connections share one use.
    assert!(may_start(&places).unwrap(), "the connections gate is open");
    let first = listing.take().unwrap();
    let second = listing.take().unwrap();
    assert_eq!(count(&Kind::Connection.uses(&places)), 1);
    drop(hook_run);
    assert!(
        said.recv_timeout(Duration::from_millis(300)).is_err(),
        "install waits while the connections are open"
    );
    drop(first);
    assert!(
        said.recv_timeout(Duration::from_millis(300)).is_err(),
        "install waits while one connection is open"
    );
    drop(second);
    let said = said.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        said,
        vec![
            "waiting for process 11, builds checks run, since 10:00:00".to_string(),
            "waiting for process 12, builds item list, since 10:00:01".to_string(),
        ]
    );
    let closed = closer.join().unwrap();
    // With both gates closed, a connection waits until install ends.
    let (opened_tx, opened_rx) = channel();
    let opener = {
        let places = places.clone();
        std::thread::spawn(move || {
            let listing = PerConnection::new(places, 14, "item list", "10:00:03");
            let taken = listing.take().map(drop);
            opened_tx.send(taken.is_ok()).unwrap();
        })
    };
    assert!(
        opened_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "the connection waits at the closed gate"
    );
    drop(closed);
    assert!(ran_rx.recv_timeout(Duration::from_secs(10)).unwrap());
    assert!(opened_rx.recv_timeout(Duration::from_secs(10)).unwrap());
    waiter.join().unwrap();
    opener.join().unwrap();
    assert_eq!(
        count(&Kind::Command.uses(&places)),
        0,
        "every use removed its file"
    );
    assert_eq!(
        count(&Kind::Connection.uses(&places)),
        0,
        "every use removed its file"
    );
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
