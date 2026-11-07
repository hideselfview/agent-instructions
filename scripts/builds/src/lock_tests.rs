use super::*;
use crate::testing::Scratch;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(200);

fn places(scratch: &Scratch) -> Places {
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.app_checkout_file("demo"), "side-1\n").unwrap();
    places
}

/// Takes the lock on a thread of its own, saying <name> once it holds it, and
/// frees it when told to.
fn take_on_thread(places: &Places, name: &'static str, said: std::sync::mpsc::Sender<&'static str>) -> std::sync::mpsc::Sender<()> {
    let (free, freed): (_, Receiver<()>) = channel();
    let places = places.clone();
    std::thread::spawn(move || {
        let held = take(&places, "demo").unwrap();
        said.send(name).unwrap();
        freed.recv().unwrap();
        held.release();
    });
    free
}

/// Waits until <queue> holds <count> tickets.
fn until_tickets(queue: &Path, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while tickets(queue).unwrap().len() < count {
        assert!(Instant::now() < deadline, "{count} tickets never appeared");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn waiters_take_the_lock_in_the_order_they_came() {
    let scratch = Scratch::new("lock-order");
    let places = places(&scratch);
    let queue = places.root.join("demo/app.queue");
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
fn a_waiter_gone_from_the_queue_lets_no_one_past_the_holder() {
    let scratch = Scratch::new("lock-gone");
    let places = places(&scratch);
    let queue = places.root.join("demo/app.queue");
    let (said, heard) = channel();
    let free_first = take_on_thread(&places, "first", said.clone());
    assert_eq!(heard.recv().unwrap(), "first");
    // A ticket nothing holds: its process died waiting.
    std::fs::write(queue.join("2"), "").unwrap();
    let free_third = take_on_thread(&places, "third", said);
    assert!(heard.recv_timeout(SETTLE).is_err(), "the holder still holds the lock");
    assert!(!queue.join("2").exists(), "the dead ticket is taken away");

    free_first.send(()).unwrap();
    assert_eq!(heard.recv().unwrap(), "third");
    free_third.send(()).unwrap();
}

#[test]
fn only_a_held_ticket_counts_as_ahead() {
    let scratch = Scratch::new("lock-ahead");
    let queue = scratch.dir.join("app.queue");
    std::fs::create_dir_all(&queue).unwrap();
    std::fs::write(queue.join("1"), "").unwrap();
    let (_, first, ahead) = join_queue(&queue).unwrap();
    assert_eq!(ahead, 0, "a ticket left by a finished process is no one");
    let (number, second, ahead) = join_queue(&queue).unwrap();
    assert_eq!((number, ahead), (3, 1));
    unsafe {
        libc::close(first);
        libc::close(second);
    }
}
