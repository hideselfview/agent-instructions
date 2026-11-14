use super::*;
use crate::lane::Lane;
use crate::testing::Scratch;
use std::sync::mpsc::{Receiver, channel};
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
fn take_on_thread(
    places: &Places,
    name: &'static str,
    said: std::sync::mpsc::Sender<&'static str>,
) -> std::sync::mpsc::Sender<()> {
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
    assert!(
        heard.recv_timeout(SETTLE).is_err(),
        "only one holds the lock"
    );
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
    assert_eq!(
        run_in_group(Command::new("true"), "true").unwrap(),
        Ran::Finished
    );
    assert!(matches!(
        run_in_group(Command::new("false"), "false"),
        Err(Fail::Exit(1))
    ));
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
    assert_eq!(
        held,
        ["side-1", "side-2"],
        "each build has a checkout of its own"
    );

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

#[test]
fn a_capturing_run_keeps_what_the_command_printed_on_both_streams() {
    let mut start = Command::new("sh");
    start.args(["-c", "echo out; echo err >&2; exit 4"]);
    let kept = Captured::default();
    assert!(matches!(
        run_in_group_capturing(start, "sh", None, &kept),
        Err(Fail::Exit(4))
    ));
    let text = String::from_utf8(kept.lock().unwrap().clone()).unwrap();
    assert!(text.contains("out\n") && text.contains("err\n"), "{text:?}");
}

/// A program in place of ssh that answers a host probe with <load> on one
/// core and runs nothing else.
fn ssh_with_load(dir: &std::path::Path, load: &str) -> Ssh {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        format!("#!/bin/sh\nprintf 'cores=1\\nload={load} 0 0\\nfree_kib=900000000\\n'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    Ssh {
        program: program.display().to_string(),
    }
}

fn places_with_host(scratch: &Scratch, entries: &str) -> Places {
    let places = places(scratch);
    // This Mac's free disk is read from its home.
    std::fs::create_dir_all(&places.home).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::Android), entries).unwrap();
    let ssh = ssh_with_load(&scratch.dir.join("add"), "0.00");
    crate::host::add(&places, &ssh, "m1", "user@m1").unwrap();
    places
}

#[test]
fn a_build_that_runs_here_never_takes_a_checkout_on_a_host() {
    let scratch = Scratch::new("lock-here");
    let places = places_with_host(&scratch, "m1:app-1\n");
    let ssh = ssh_with_load(&scratch.dir.join("ssh"), "0.00");
    let Err(error) = take_in(&places, &ssh, "demo", Lane::Android, Where::Here) else {
        panic!("only a host's checkout is recorded");
    };
    assert!(error.text().contains("on this Mac"));
    let held = take_where(&places, &ssh, "demo", Lane::Android, Where::Anywhere).unwrap();
    assert_eq!(held.checkout, "m1:app-1");
    held.release();
}

#[test]
fn the_checkout_on_the_less_loaded_host_goes_first() {
    let scratch = Scratch::new("lock-load");
    let places = places_with_host(&scratch, "side-2\nm1:app-1\n");
    // This Mac's load per core is never 1000.
    let busy = ssh_with_load(&scratch.dir.join("busy"), "1000.00");
    let idle = ssh_with_load(&scratch.dir.join("idle"), "0.00");
    let entries = || vec!["side-2".to_string(), "m1:app-1".to_string()];
    let mut loads = Loads {
        places: &places,
        ssh: &busy,
        asked: HashMap::new(),
    };
    let (ordered, skipped) = by_load(&mut loads, entries());
    assert_eq!(ordered, vec!["side-2", "m1:app-1"], "the host is busier");
    assert!(skipped.is_empty());
    let mut loads = Loads {
        places: &places,
        ssh: &idle,
        asked: HashMap::new(),
    };
    let (ordered, _) = by_load(&mut loads, entries());
    assert_eq!(
        ordered,
        vec!["m1:app-1", "side-2"],
        "the host's 0 per core is under this Mac's"
    );
}

#[test]
fn a_host_that_does_not_answer_is_skipped_and_named_when_it_was_the_only_one() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("lock-unreachable");
    let places = places_with_host(&scratch, "m1:app-1\n");
    let dir = scratch.dir.join("down");
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        "#!/bin/sh\necho 'ssh: connect to host m1 port 22: Connection refused' >&2\nexit 255\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let down = Ssh {
        program: program.display().to_string(),
    };
    let Err(error) = take_where(&places, &down, "demo", Lane::Android, Where::Anywhere) else {
        panic!("the only checkout is on a host that is down");
    };
    let text = error.text();
    assert!(
        text.contains("m1:app-1") && text.contains("Connection refused"),
        "{text}"
    );
}
