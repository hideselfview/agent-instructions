use super::*;
use crate::lane::Lane;
use crate::lock::Where;
use crate::testing::Scratch;
use std::sync::mpsc::channel;
use std::time::Duration;

const SETTLE: Duration = Duration::from_millis(300);

/// The lane's queue with one app checkout, so one build goes at a time.
fn queue(scratch: &Scratch) -> Queue {
    let queue = Queue::in_repo_dir(&scratch.dir, Lane::Mac);
    set_slots(&queue, 1);
    queue
}

/// Plants a ticket that nothing holds, last in line, as one left by a process
/// that died waiting.
fn plant_dead(queue: &Queue, name: &str) {
    let _guard = queue.guard().unwrap();
    std::fs::write(queue.ticket_path(name), "").unwrap();
    let mut order = queue.order().unwrap();
    order.push(name.to_string());
    queue.write_order(&order).unwrap();
}

#[test]
fn health_reads_waits_from_unused_and_held_app_checkouts() {
    let scratch = Scratch::new("queue-health-waits");
    let queue = queue_with(&scratch, &["side-1", "m1:side-1", "side-2"]);
    let local = hold(&queue, "side-1", 10);
    let remote = open_locked(&queue.lock_path("m1:side-1")).unwrap();
    let tickets = [
        plant_live(&queue, 30, Where::Here),
        plant_live(&queue, 40, Where::Here),
        plant_live(&queue, 50, Where::Anywhere),
    ];
    let waits = || {
        queue
            .app_waits()
            .unwrap()
            .into_iter()
            .collect::<BTreeMap<_, _>>()
    };
    let first = waits();
    assert!(
        first[&30].is_empty(),
        "one local checkout has never been used"
    );
    assert!(first[&40].contains(&AppWait::Process(30)));
    assert!(first[&40].contains(&AppWait::Process(10)));
    assert!(first[&50].contains(&AppWait::HeldCheckout("m1:side-1".into())));

    unsafe { libc::close(remote) };
    assert!(
        waits()[&50].is_empty(),
        "the anywhere build can use the free remote checkout"
    );
    queue
        .record_checkout_wait(&tickets[2].name, &["side-1".to_string()])
        .unwrap();
    assert_eq!(waits()[&50], [AppWait::Process(10)]);
    drop(tickets);
    unsafe { libc::close(local) };
}

#[test]
fn a_waiter_that_can_only_pass_on_a_remote_checkout_does_not_take_a_local_waiters_place() {
    let scratch = Scratch::new("queue-health-remote-pass");
    let queue = queue_with(&scratch, &["side-1", "m1:side-1"]);
    let local = hold(&queue, "side-1", 10);
    let tickets = [
        plant_live(&queue, 20, Where::Here),
        plant_live(&queue, 30, Where::Anywhere),
        plant_live(&queue, 40, Where::Here),
    ];
    let found: BTreeMap<_, _> = queue.app_waits().unwrap().into_iter().collect();
    drop(tickets);
    unsafe { libc::close(local) };
    assert!(found[&30].is_empty());
    assert!(found[&40].contains(&AppWait::Process(20)), "{found:?}");
    assert!(!found[&40].contains(&AppWait::Process(30)), "{found:?}");
}

#[test]
fn an_unreadable_order_leaves_no_ticket() {
    let scratch = Scratch::new("queue-order-unreadable");
    let queue = queue(&scratch);
    let guard = queue.guard().unwrap();
    std::fs::create_dir(queue.dir.join("order")).unwrap();
    drop(guard);
    assert!(queue.join(Where::Here).is_err());
    let files: std::collections::BTreeSet<_> = std::fs::read_dir(&queue.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files, [".next".into(), "order".into()].into());
}

#[test]
fn a_ticket_names_its_process_and_counts_its_own() {
    let scratch = Scratch::new("queue-names");
    let queue = queue(&scratch);
    let (first, ahead) = queue.join(Where::Anywhere).unwrap();
    assert_eq!(ahead, 0);
    let (second, ahead) = queue.join(Where::Anywhere).unwrap();
    assert_eq!(ahead, 1);
    assert_eq!(
        queue.order().unwrap(),
        vec![first.name.clone(), second.name.clone()]
    );
    assert_eq!(pid_of(&first.name), Some(std::process::id() as i32));
    drop(first);
    drop(second);
}

#[test]
fn a_reissued_ticket_takes_the_old_ones_place() {
    let scratch = Scratch::new("queue-reissue");
    let queue = queue(&scratch);
    let (first, _) = queue.join(Where::Anywhere).unwrap();
    let (second, _) = queue.join(Where::Anywhere).unwrap();
    let (third, _) = queue.join(Where::Anywhere).unwrap();
    let new = queue.reissue(&second).unwrap();
    assert_eq!(
        queue.order().unwrap(),
        vec![first.name.clone(), new.name.clone(), third.name.clone()]
    );
    assert!(
        !queue.ticket_path(&second.name).exists(),
        "the old ticket's file is gone"
    );
    drop((first, second, third, new));
}

#[test]
fn a_waiter_moves_to_right_behind_the_holder() {
    let scratch = Scratch::new("queue-top");
    let queue = queue(&scratch);
    for name in [
        "10-1-anywhere",
        "20-1-anywhere",
        "30-1-anywhere",
        "40-1-anywhere",
    ] {
        plant_dead(&queue, name);
    }
    let _held = hold(&queue, "side-1", 10);
    queue.move_to_top(40).unwrap();
    assert_eq!(
        queue.order().unwrap(),
        vec![
            "10-1-anywhere",
            "40-1-anywhere",
            "20-1-anywhere",
            "30-1-anywhere"
        ]
    );
    assert!(queue.move_to_top(10).is_err(), "the holder holds a lock");
    assert!(queue.move_to_top(99).is_err(), "no such waiter");
}

#[test]
fn a_dead_ticket_in_line_lets_no_one_past_the_holder() {
    let scratch = Scratch::new("queue-dead");
    let queue = queue(&scratch);
    let (holder, _) = queue.join(Where::Anywhere).unwrap();
    plant_dead(&queue, "1-1-anywhere");
    let (last, _) = queue.join(Where::Anywhere).unwrap();
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
        queue.order().unwrap(),
        vec![holder.name.clone(), last.name.clone()],
        "the dead ticket is taken away"
    );
    assert!(!queue.ticket_path("1-1-anywhere").exists());
    drop(holder);
    finished.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(queue.order().unwrap(), vec![last.name.clone()]);
    drop(last);
}

#[test]
fn a_signal_between_reading_the_order_and_waiting_is_not_lost() {
    let scratch = Scratch::new("queue-gap");
    let queue = queue(&scratch);
    let (first, _) = queue.join(Where::Anywhere).unwrap();
    let (second, _) = queue.join(Where::Anywhere).unwrap();
    let (last, _) = queue.join(Where::Anywhere).unwrap();
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
    drop(first);
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the wait was re-pointed to the first ticket");
    drop(second);
    drop(last);
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
}

#[test]
fn the_waiting_are_the_tickets_whose_builds_hold_no_lock_wherever_they_stand() {
    let scratch = Scratch::new("queue-waiting");
    let queue = queue_with(&scratch, &["side-1", "side-2", "side-3"]);
    let tickets: Vec<Ticket> = [10, 20, 30, 40, 50]
        .into_iter()
        .map(|pid| plant_live(&queue, pid, Where::Anywhere))
        .collect();
    // The builds of the first, the third and the fifth ticket hold locks.
    let held = [
        hold(&queue, "side-1", 10),
        hold(&queue, "side-2", 30),
        hold(&queue, "side-3", 50),
    ];
    assert_eq!(
        queue.waiting().unwrap(),
        vec![(20, Where::Anywhere), (40, Where::Anywhere)]
    );
    for fd in held {
        unsafe { libc::close(fd) };
    }
    drop(tickets);
}

#[test]
fn as_many_waiters_as_slots_go_at_once_and_the_next_goes_when_any_of_them_leaves() {
    let scratch = Scratch::new("queue-two-slots");
    let queue = queue(&scratch);
    set_slots(&queue, 2);
    let (first, _) = queue.join(Where::Anywhere).unwrap();
    let (second, _) = queue.join(Where::Anywhere).unwrap();
    let (third, _) = queue.join(Where::Anywhere).unwrap();
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
    drop(first);
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the third took the slot the first left");
    assert_eq!(
        queue.order().unwrap(),
        vec![second.name.clone(), third.name.clone()]
    );
    drop(second);
    drop(third);
}

#[test]
fn a_waiter_moves_to_right_behind_the_holders() {
    let scratch = Scratch::new("queue-top-slots");
    let queue = queue(&scratch);
    set_slots(&queue, 2);
    for name in [
        "10-1-anywhere",
        "20-1-anywhere",
        "30-1-anywhere",
        "40-1-anywhere",
    ] {
        plant_dead(&queue, name);
    }
    // Both locks are held.
    let held = [hold(&queue, "side-1", 10), hold(&queue, "side-2", 20)];
    queue.move_to_top(40).unwrap();
    assert_eq!(
        queue.order().unwrap(),
        vec![
            "10-1-anywhere",
            "20-1-anywhere",
            "40-1-anywhere",
            "30-1-anywhere"
        ]
    );
    assert!(queue.move_to_top(20).is_err(), "a holder holds a lock");
    for fd in held {
        unsafe { libc::close(fd) };
    }
}

#[test]
fn with_a_lock_free_a_waiter_moves_in_behind_the_builds_that_hold_one() {
    let scratch = Scratch::new("queue-top-free");
    let queue = queue(&scratch);
    set_slots(&queue, 2);
    for name in [
        "10-1-anywhere",
        "20-1-anywhere",
        "30-1-anywhere",
        "40-1-anywhere",
    ] {
        plant_dead(&queue, name);
    }
    // One lock held: the waiter goes second, where it may take the free one.
    let held = hold(&queue, "side-1", 10);
    queue.move_to_top(40).unwrap();
    assert_eq!(
        queue.order().unwrap(),
        vec![
            "10-1-anywhere",
            "40-1-anywhere",
            "20-1-anywhere",
            "30-1-anywhere"
        ]
    );
    unsafe { libc::close(held) };
}

/// The lane's queue with the checkouts <entries>, in the order given.
fn queue_with(scratch: &Scratch, entries: &[&str]) -> Queue {
    let queue = Queue::in_repo_dir(&scratch.dir, Lane::Mac);
    let text: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
    std::fs::write(&queue.checkouts_file, text).unwrap();
    queue
}

/// A ticket of the process <pid> that stays locked, last in line, as a
/// waiter or a holder of another process has it. The test closes it.
fn plant_live(queue: &Queue, pid: i32, allowed: Where) -> Ticket {
    plant_live_named(queue, format!("{pid}-1-{}", allowed.word()))
}

/// `plant_live` with the whole name given, as an earlier builds named it.
fn plant_live_named(queue: &Queue, name: String) -> Ticket {
    let _guard = queue.guard().unwrap();
    let ticket = Ticket::locked(name.clone(), &queue.ticket_path(&name)).unwrap();
    let mut order = queue.order().unwrap();
    order.push(name.clone());
    queue.write_order(&order).unwrap();
    ticket
}

/// Takes the lock of <checkout> and writes the record of the run of the
/// process <pid> beside it, as the holder's `Held` does.
fn hold(queue: &Queue, checkout: &str, pid: i32) -> libc::c_int {
    std::fs::create_dir_all(&queue.locks_dir).unwrap();
    let lock = try_lock(&queue.lock_path(checkout)).unwrap().unwrap();
    crate::holder_run::HolderRun {
        pid,
        joined_at: 0,
        waited_ms: 0,
        step: None,
    }
    .write(&crate::places::run_file_in(&queue.locks_dir, checkout))
    .unwrap();
    lock
}

#[test]
fn a_waiter_that_may_take_any_checkout_waits_behind_an_earlier_one_that_may_too() {
    let scratch = Scratch::new("queue-first-fit-any");
    let queue = queue_with(&scratch, &["m1:mac-1", "side-3"]);
    let a = plant_live(&queue, 10, Where::Anywhere);
    let _held = hold(&queue, "m1:mac-1", 10);
    let first = plant_live(&queue, 20, Where::Anywhere);
    let second = plant_live(&queue, 30, Where::Here);
    // side-3 is the one free checkout: the earlier waiter may take it.
    assert!(
        queue
            .may_go(&first.name, &queue.order().unwrap()[..1])
            .unwrap()
    );
    assert!(
        !queue
            .may_go(&second.name, &queue.order().unwrap()[..2])
            .unwrap()
    );
    drop((a, first, second));
}

#[test]
fn each_free_checkout_is_left_for_one_waiter_that_may_use_it() {
    let scratch = Scratch::new("queue-first-fit-counts");
    let queue = queue_with(&scratch, &["m1:mac-1", "side-3", "side-4"]);
    let a = plant_live(&queue, 10, Where::Anywhere);
    let _held = hold(&queue, "m1:mac-1", 10);
    let tickets: Vec<Ticket> = [20, 30, 40]
        .into_iter()
        .map(|pid| plant_live(&queue, pid, Where::Here))
        .collect();
    // Two checkouts here are free: the first two waiters here take them.
    assert!(
        queue
            .may_go(&tickets[0].name, &queue.order().unwrap()[..1])
            .unwrap()
    );
    assert!(
        queue
            .may_go(&tickets[1].name, &queue.order().unwrap()[..2])
            .unwrap()
    );
    assert!(
        !queue
            .may_go(&tickets[2].name, &queue.order().unwrap()[..3])
            .unwrap()
    );
    drop(a);
    drop(tickets);
}

#[test]
fn a_ticket_name_says_where_its_build_may_run_and_a_reissued_one_keeps_it() {
    let scratch = Scratch::new("queue-ticket-where");
    let queue = queue(&scratch);
    let (ticket, _) = queue.join(Where::Here).unwrap();
    assert_eq!(allowed_of(&ticket.name).unwrap(), Where::Here);
    let again = queue.reissue(&ticket).unwrap();
    assert_eq!(allowed_of(&again.name).unwrap(), Where::Here);
    assert_eq!(pid_of(&again.name), Some(std::process::id() as i32));
    assert!(allowed_of("12-1").is_err());
    drop(ticket);
    drop(again);
}

#[test]
fn a_dead_ticket_of_an_earlier_builds_is_dropped_and_lets_the_waiter_go() {
    let scratch = Scratch::new("queue-dead-old-name");
    let queue = queue(&scratch);
    plant_dead(&queue, "85902-1-hosts");
    let (mine, _) = queue.join(Where::Anywhere).unwrap();
    queue.wait_for_turn(&mine.name).unwrap();
    assert_eq!(queue.order().unwrap(), vec![mine.name.clone()]);
    assert!(!queue.ticket_path("85902-1-hosts").exists());
    drop(mine);
}

#[test]
fn an_order_entry_with_no_ticket_file_is_dropped() {
    let scratch = Scratch::new("queue-entry-no-file");
    let queue = queue(&scratch);
    std::fs::create_dir_all(&queue.dir).unwrap();
    queue.write_order(&["85902-1-hosts".to_string()]).unwrap();
    let (mine, _) = queue.join(Where::Anywhere).unwrap();
    queue.wait_for_turn(&mine.name).unwrap();
    assert_eq!(queue.order().unwrap(), vec![mine.name.clone()]);
    drop(mine);
}

#[test]
fn a_live_ticket_of_an_earlier_builds_is_a_build_to_wait_behind() {
    let scratch = Scratch::new("queue-live-old-name");
    let queue = queue(&scratch);
    let old = plant_live_named(&queue, "85902-1-hosts".to_string());
    let (mine, _) = queue.join(Where::Anywhere).unwrap();
    let (done, finished) = channel();
    let (reached, reads) = channel();
    let waiting = queue.clone();
    let name = mine.name.clone();
    std::thread::spawn(move || {
        let result = waiting
            .wait_for_turn_with(&name, || {
                let _ = reached.send(());
            })
            .map_err(|_| "wait failed");
        done.send(result).unwrap();
    });
    // The waiter read the order, found the live ticket and started to wait.
    reads.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        finished.try_recv().is_err(),
        "the waiter neither fails nor goes ahead of the live ticket"
    );
    drop(old);
    finished
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(queue.order().unwrap(), vec![mine.name.clone()]);
    drop(mine);
}
