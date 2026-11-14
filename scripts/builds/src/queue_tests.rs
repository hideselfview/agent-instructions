use super::*;
use crate::lane::Lane;
use crate::testing::Scratch;
use std::sync::mpsc::channel;
use std::time::Duration;

const SETTLE: Duration = Duration::from_millis(300);

fn queue(scratch: &Scratch) -> Queue {
    Queue::in_repo_dir(&scratch.dir, Lane::Mac)
}

/// Plants a ticket that nothing holds, last in line, as one left by a process
/// that died waiting.
fn plant_dead(queue: &Queue, name: &str) {
    let _guard = queue.guard().unwrap();
    std::fs::write(queue.ticket_path(name), "").unwrap();
    let mut order = queue.order();
    order.push(name.to_string());
    queue.write_order(&order).unwrap();
}

fn close(ticket: &Ticket) {
    unsafe { libc::close(ticket.fd) };
}

#[test]
fn a_ticket_names_its_process_and_counts_its_own() {
    let scratch = Scratch::new("queue-names");
    let queue = queue(&scratch);
    let (first, ahead) = queue.join().unwrap();
    assert_eq!(ahead, 0);
    let (second, ahead) = queue.join().unwrap();
    assert_eq!(ahead, 1);
    assert_eq!(queue.order(), vec![first.name.clone(), second.name.clone()]);
    assert_eq!(pid_of(&first.name), Some(std::process::id() as i32));
    assert_eq!(
        queue.waiting(),
        vec![std::process::id() as i32],
        "the head is the holder, not waiting"
    );
    close(&first);
    close(&second);
}

#[test]
fn a_reissued_ticket_takes_the_old_ones_place() {
    let scratch = Scratch::new("queue-reissue");
    let queue = queue(&scratch);
    let (first, _) = queue.join().unwrap();
    let (second, _) = queue.join().unwrap();
    let (third, _) = queue.join().unwrap();
    let new = queue.reissue(&second).unwrap();
    assert_eq!(
        queue.order(),
        vec![first.name.clone(), new.name.clone(), third.name.clone()]
    );
    assert!(
        !queue.ticket_path(&second.name).exists(),
        "the old ticket's file is gone"
    );
    for ticket in [&first, &second, &third, &new] {
        close(ticket);
    }
}

#[test]
fn a_waiter_moves_to_right_behind_the_holder() {
    let scratch = Scratch::new("queue-top");
    let queue = queue(&scratch);
    for name in ["10-1", "20-1", "30-1", "40-1"] {
        plant_dead(&queue, name);
    }
    queue.move_to_top(40).unwrap();
    assert_eq!(queue.order(), vec!["10-1", "40-1", "20-1", "30-1"]);
    assert!(
        queue.move_to_top(10).is_err(),
        "the holder is already first"
    );
    assert!(queue.move_to_top(99).is_err(), "no such waiter");
}

#[test]
fn a_dead_ticket_in_line_lets_no_one_past_the_holder() {
    let scratch = Scratch::new("queue-dead");
    let queue = queue(&scratch);
    let (holder, _) = queue.join().unwrap();
    plant_dead(&queue, "1-1");
    let (last, _) = queue.join().unwrap();
    let (done, finished) = channel();
    let waiting = queue.clone();
    let name = last.name.clone();
    std::thread::spawn(move || {
        waiting.wait_for_turn(&name).unwrap();
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(SETTLE).is_err(),
        "the holder's ticket is still held"
    );
    assert_eq!(
        queue.order(),
        vec![holder.name.clone(), last.name.clone()],
        "the dead ticket is taken away"
    );
    assert!(!queue.ticket_path("1-1").exists());
    close(&holder);
    finished.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(queue.order(), vec![last.name.clone()]);
    close(&last);
}

#[test]
fn a_signal_between_reading_the_order_and_waiting_is_not_lost() {
    let scratch = Scratch::new("queue-gap");
    let queue = queue(&scratch);
    let (first, _) = queue.join().unwrap();
    let (second, _) = queue.join().unwrap();
    let (last, _) = queue.join().unwrap();
    let waiting = queue.clone();
    let (done, finished) = channel();
    let name = last.name.clone();
    let reorder = vec![first.name.clone(), last.name.clone(), second.name.clone()];
    std::thread::spawn(move || {
        let mut sent = false;
        waiting
            .wait_for_turn_with(&name, || {
                // The order changes and its signal comes in the gap: the wait
                // on the second ticket is already started, no event waited
                // for yet.
                if !std::mem::replace(&mut sent, true) {
                    let _guard = waiting.guard().unwrap();
                    waiting.write_order(&reorder).unwrap();
                    unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
                }
            })
            .unwrap();
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(SETTLE).is_err(),
        "the first ticket is still held"
    );
    // Waiting on the second ticket, still held, would never end.
    close(&first);
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the wait was re-pointed to the first ticket");
    close(&second);
    close(&last);
}

/// Records <count> app checkouts for the queue's lane, so it has that many
/// slots.
fn set_slots(queue: &Queue, count: usize) {
    let text: String = (1..=count).map(|n| format!("side-{n}\n")).collect();
    std::fs::write(&queue.checkouts_file, text).unwrap();
}

#[test]
fn a_queue_has_a_slot_for_each_app_checkout_and_at_least_one() {
    let scratch = Scratch::new("queue-slots");
    let queue = queue(&scratch);
    assert_eq!(queue.slots(), 1);
    set_slots(&queue, 3);
    assert_eq!(queue.slots(), 3);
    let tickets: Vec<_> = (0..5).map(|_| queue.join().unwrap().0).collect();
    assert_eq!(
        queue.waiting().len(),
        2,
        "the first three hold or may take a lock"
    );
    for ticket in &tickets {
        close(ticket);
    }
}

#[test]
fn as_many_waiters_as_slots_go_at_once_and_the_next_goes_when_any_of_them_leaves() {
    let scratch = Scratch::new("queue-two-slots");
    let queue = queue(&scratch);
    set_slots(&queue, 2);
    let (first, _) = queue.join().unwrap();
    let (second, _) = queue.join().unwrap();
    let (third, _) = queue.join().unwrap();
    queue.wait_for_turn(&first.name).unwrap();
    queue.wait_for_turn(&second.name).unwrap();
    let (done, finished) = channel();
    let waiting = queue.clone();
    let name = third.name.clone();
    std::thread::spawn(move || {
        waiting.wait_for_turn(&name).unwrap();
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(SETTLE).is_err(),
        "both slots are taken"
    );
    // The first leaves, not the third's predecessor.
    close(&first);
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the third took the slot the first left");
    assert_eq!(queue.order(), vec![second.name.clone(), third.name.clone()]);
    close(&second);
    close(&third);
}

#[test]
fn a_waiter_moves_to_right_behind_the_holders() {
    let scratch = Scratch::new("queue-top-slots");
    let queue = queue(&scratch);
    set_slots(&queue, 2);
    for name in ["10-1", "20-1", "30-1", "40-1"] {
        plant_dead(&queue, name);
    }
    // Both locks are held.
    let locks = [queue.lock_path("side-1"), queue.lock_path("side-2")];
    std::fs::create_dir_all(locks[0].parent().unwrap()).unwrap();
    let held: Vec<_> = locks
        .iter()
        .map(|lock| open_with(lock, libc::O_CREAT | libc::O_CLOEXEC).unwrap())
        .collect();
    queue.move_to_top(40).unwrap();
    assert_eq!(queue.order(), vec!["10-1", "20-1", "40-1", "30-1"]);
    assert!(
        queue.move_to_top(20).is_err(),
        "a holder is already among the first"
    );
    for fd in held {
        unsafe { libc::close(fd) };
    }
}

#[test]
fn with_a_lock_free_a_waiter_moves_in_behind_the_builds_that_hold_one() {
    let scratch = Scratch::new("queue-top-free");
    let queue = queue(&scratch);
    set_slots(&queue, 2);
    for name in ["10-1", "20-1", "30-1", "40-1"] {
        plant_dead(&queue, name);
    }
    // One lock held: the waiter goes second, where it may take the free one.
    let lock = queue.lock_path("side-1");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = open_with(&lock, libc::O_CREAT | libc::O_CLOEXEC).unwrap();
    queue.move_to_top(40).unwrap();
    assert_eq!(queue.order(), vec!["10-1", "40-1", "20-1", "30-1"]);
    unsafe { libc::close(held) };
}
