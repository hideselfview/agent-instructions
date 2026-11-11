use super::*;
use crate::lane::Lane;
use crate::testing::Scratch;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(200);

fn places(scratch: &Scratch) -> Places {
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.app_checkout_file("demo", Lane::Mac), "side-1\n").unwrap();
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
