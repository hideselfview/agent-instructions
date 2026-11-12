use super::*;
use crate::lane::Lane;
use crate::testing::Scratch;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(200);

fn places(scratch: &Scratch) -> Places {
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::Mac), "side-1\n").unwrap();
    places
}

/// Takes the lock on a thread of its own, saying <name> once it holds it, and
/// frees it when told to.
fn take_on_thread(places: &Places, name: &'static str, said: std::sync::mpsc::Sender<&'static str>) -> std::sync::mpsc::Sender<()> {
    let (free, freed): (_, Receiver<()>) = channel();
    let places = places.clone();
    std::thread::spawn(move || {
        let held = take(&places, "demo", Lane::Mac).unwrap();
        said.send(name).unwrap();
        freed.recv().unwrap();
        held.release();
    });
    free
}

/// Waits until the queue holds <count> tickets.
fn until_tickets(queue: &Queue, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while queue.order().len() < count {
        assert!(Instant::now() < deadline, "{count} tickets never appeared");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn waiters_take_the_lock_in_the_order_they_came() {
    let scratch = Scratch::new("lock-order");
    let places = places(&scratch);
    let queue = Queue::of(&places, "demo", Lane::Mac);
    let (said, heard) = channel();
    let free_first = take_on_thread(&places, "first", said.clone());
    assert_eq!(heard.recv().unwrap(), "first");
    let free_second = take_on_thread(&places, "second", said.clone());
    until_tickets(&queue, 2);
    let free_third = take_on_thread(&places, "third", said.clone());
    until_tickets(&queue, 3);
    let free_fourth = take_on_thread(&places, "fourth", said);
    until_tickets(&queue, 4);

    free_first.send(()).unwrap();
    assert_eq!(heard.recv().unwrap(), "second");
    assert!(heard.recv_timeout(SETTLE).is_err(), "only one holds the lock");
    free_second.send(()).unwrap();
    assert_eq!(heard.recv().unwrap(), "third");
    free_third.send(()).unwrap();
    assert_eq!(heard.recv().unwrap(), "fourth");
    free_fourth.send(()).unwrap();
}

#[test]
fn a_build_in_a_group_finishes_or_fails_with_its_exit_status() {
    let scratch = Scratch::new("lock-ran");
    let places = places(&scratch);
    let held = take(&places, "demo", Lane::Mac).unwrap();
    assert_eq!(run_in_group(Command::new("true"), "true").unwrap(), Ran::Finished);
    assert!(matches!(run_in_group(Command::new("false"), "false"), Err(Fail::Exit(1))));
    held.release();
}

/// Records <count> app checkouts in the mac lane.
fn set_of(places: &Places, count: usize) {
    let text: String = (1..=count).map(|n| format!("side-{n}\n")).collect();
    std::fs::write(places.app_checkouts_file("demo", Lane::Mac), text).unwrap();
}

#[test]
fn builds_take_different_checkouts_at_once_and_wait_when_every_lock_is_held() {
    let scratch = Scratch::new("lock-set");
    let places = places(&scratch);
    set_of(&places, 2);
    let first = take(&places, "demo", Lane::Mac).unwrap();
    let second = take(&places, "demo", Lane::Mac).unwrap();
    let mut held = [first.checkout.clone(), second.checkout.clone()];
    held.sort();
    assert_eq!(held, ["side-1", "side-2"], "each build has a checkout of its own");

    let (said, heard) = channel();
    let free_third = take_on_thread(&places, "third", said);
    assert!(heard.recv_timeout(SETTLE).is_err(), "no lock is free");
    // The second to join leaves: the third has its checkout.
    let freed = second.checkout.clone();
    second.release();
    assert_eq!(heard.recv().unwrap(), "third");
    assert!(places.app_lock_file("demo", Lane::Mac, &freed).is_file());
    free_third.send(()).unwrap();
    first.release();
}
