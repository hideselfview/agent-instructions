use super::*;
use crate::lane::Lane;
use crate::testing::Scratch;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(200);

fn places(scratch: &Scratch) -> Places {
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::MAC), "app-1\n").unwrap();
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
        let held = take(&places, &crate::testing::no_ssh(), "demo", Lane::MAC).unwrap();
        said.send(name).unwrap();
        freed.recv().unwrap();
        held.release();
    });
    free
}

/// Waits until the queue holds <count> tickets.
fn until_tickets(queue: &Queue, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while queue.order().unwrap().len() < count {
        assert!(Instant::now() < deadline, "{count} tickets never appeared");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn waiters_take_the_lock_in_the_order_they_came() {
    let scratch = Scratch::new("lock-order");
    let places = places(&scratch);
    let queue = Queue::of(&places, "demo", Lane::MAC);
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
    let held = take(&places, &crate::testing::no_ssh(), "demo", Lane::MAC).unwrap();
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

#[test]
fn an_android_step_can_leave_a_daemon_without_holding_the_checkout() {
    use std::io::Read;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;

    let scratch = Scratch::new("lock-daemon");
    let places = places(&scratch);
    let lane = Lane::ANDROID;
    std::fs::write(places.app_checkouts_file("demo", lane), "app-1\n").unwrap();
    let held = take(&places, &crate::testing::no_ssh(), "demo", lane).unwrap();
    let lock = places.app_lock_file("demo", lane, &held.checkout);
    let ticket = held.queue.ticket_path(&held.ticket.name);

    // The step exits after forking. The daemon starts a new session and stays
    // alive until the test closes its socket, including if an assertion fails.
    let (mut test, daemon) = UnixStream::pair().unwrap();
    test.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut step = Command::new("python3");
    step.args([
        "-c",
        r#"
import os

if os.fork() == 0:
    os.setsid()
    os.write(0, b"R")
    os.read(0, 1)
"#,
    ])
    .stdin(Stdio::from(OwnedFd::from(daemon)))
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    held.runs("android-step").unwrap();
    assert_eq!(
        held.checkout_lock()
            .run(step, |step| run_in_group_within(
                step,
                "android-step",
                Some(Duration::from_secs(10))
            ))
            .unwrap(),
        Within::Ran(Ran::Finished)
    );
    let mut ready = [0];
    test.read_exact(&mut ready).unwrap();
    assert_eq!(ready, *b"R");
    assert!(crate::flock::is_held(&lock).unwrap());
    held.release();

    assert!(
        !crate::flock::is_held(&lock).unwrap(),
        "the daemon kept the checkout lock"
    );
    assert!(
        !crate::flock::is_held(&ticket).unwrap(),
        "the daemon kept the queue ticket locked"
    );
}

/// Records <count> app checkouts in the mac lane.
fn set_of(places: &Places, count: usize) {
    let text: String = (1..=count).map(|n| format!("app-{n}\n")).collect();
    std::fs::write(places.app_checkouts_file("demo", Lane::MAC), text).unwrap();
}

#[test]
fn builds_take_different_checkouts_at_once_and_wait_when_every_lock_is_held() {
    let scratch = Scratch::new("lock-set");
    let places = places(&scratch);
    set_of(&places, 2);
    let first = take(&places, &crate::testing::no_ssh(), "demo", Lane::MAC).unwrap();
    let second = take(&places, &crate::testing::no_ssh(), "demo", Lane::MAC).unwrap();
    let mut held = [first.checkout.clone(), second.checkout.clone()];
    held.sort();
    assert_eq!(
        held,
        ["app-1", "app-2"],
        "each build has a checkout of its own"
    );

    let (said, heard) = channel();
    let free_third = take_on_thread(&places, "third", said);
    assert!(heard.recv_timeout(SETTLE).is_err(), "no lock is free");
    // The second to join leaves: the third has its checkout.
    let freed = second.checkout.clone();
    second.release();
    assert_eq!(heard.recv().unwrap(), "third");
    assert!(places.app_lock_file("demo", Lane::MAC, &freed).is_file());
    free_third.send(()).unwrap();
    first.release();
}

#[test]
fn a_capturing_run_keeps_what_the_command_printed_on_both_streams() {
    let mut start = Command::new("sh");
    start.args(["-c", "echo out; echo err >&2; exit 4"]);
    let kept = Captured::default();
    assert!(matches!(
        run_in_group_keeping(
            start,
            "sh",
            None,
            Keep {
                all: Some(&kept),
                line_with: None,
                each_line: None,
            }
        ),
        Err(Fail::Exit(4))
    ));
    let text = String::from_utf8(kept.lock().unwrap().clone()).unwrap();
    assert!(text.contains("out\n") && text.contains("err\n"), "{text:?}");
}

/// A watch keeps the first whole line that holds its text, from either
/// stream, and a last line with no newline counts.
#[test]
fn a_watching_run_keeps_the_first_line_that_holds_the_text() {
    let run = |script: &str| {
        let mut start = Command::new("sh");
        start.args(["-c", script]);
        let found = Found::default();
        let _ = run_in_group_keeping(
            start,
            "sh",
            None,
            Keep {
                all: None,
                line_with: Some((&["plugin failed"], &found)),
                each_line: None,
            },
        );
        found.lock().unwrap().clone()
    };
    assert_eq!(
        run(
            "echo one; echo 'A required plugin failed to load' >&2; echo 'plugin failed again' >&2; exit 1"
        ),
        Some("A required plugin failed to load".to_string())
    );
    assert_eq!(
        run("printf 'the plugin failed'"),
        Some("the plugin failed".to_string())
    );
    assert_eq!(run("echo fine"), None);
}

/// A run gives each line of both streams to its reader, a last line with no
/// newline included.
#[test]
fn a_reading_run_gives_each_line_to_its_reader() {
    let mut start = Command::new("sh");
    start.args(["-c", "echo one; echo two >&2; printf three"]);
    let read: Arc<Mutex<Vec<String>>> = Arc::default();
    let each: EachLine = {
        let read = read.clone();
        Arc::new(move |line: &str| read.lock().unwrap().push(line.to_string()))
    };
    run_in_group_keeping(
        start,
        "sh",
        None,
        Keep {
            all: None,
            line_with: None,
            each_line: Some(&each),
        },
    )
    .unwrap();
    let mut read = read.lock().unwrap().clone();
    read.sort();
    assert_eq!(read, ["one", "three", "two"]);
}

/// A watch joins a line that arrives in two reads before it looks for the
/// text in it.
#[test]
fn a_watch_joins_a_line_split_across_two_reads() {
    let found = Found::default();
    let mut watch = StreamLines {
        watch: Some((&["plugin failed"], found.clone())),
        each: None,
        line: Vec::new(),
    };
    watch.feed(b"one\nA required plu");
    assert_eq!(*found.lock().unwrap(), None);
    watch.feed(b"gin failed to load\nmore\n");
    assert_eq!(
        *found.lock().unwrap(),
        Some("A required plugin failed to load".to_string())
    );
}

/// A program in place of ssh that answers a host probe with <load> on one
/// core and runs nothing else.
fn ssh_with_load(dir: &std::path::Path, load: &str) -> Ssh {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        format!("#!/bin/sh\nprintf 'os=Darwin\\narch=arm64\\ncores=1\\nload={load} 0 0\\nfree_kib=900000000\\nmemory_free_percent=80\\nswap_used_mib=0\\nmemory_pressure_level=1\\nprocs=300\\nmax_procs=8000\\n'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    crate::testing::ssh_at(&program)
}

fn places_with_host(scratch: &Scratch, entries: &str) -> Places {
    let places = places(scratch);
    // This Mac's free disk is read from its home.
    std::fs::create_dir_all(&places.home).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::ANDROID), entries).unwrap();
    let ssh = ssh_with_load(&scratch.dir.join("add"), "0.00");
    crate::host::add(&places, &ssh, "m1", "user@m1").unwrap();
    places
}

#[test]
fn a_build_that_runs_here_never_takes_a_checkout_on_a_host() {
    let scratch = Scratch::new("lock-here");
    let places = places_with_host(&scratch, "m1:app-1\n");
    let ssh = ssh_with_load(&scratch.dir.join("ssh"), "0.00");
    let Err(error) = take_in(
        &places,
        &ssh,
        "demo",
        Lane::ANDROID,
        Takes {
            allowed: Where::Here,
            screen: false,
        },
        None,
        0,
    ) else {
        panic!("only a host's checkout is recorded");
    };
    assert!(error.text().contains("on this Mac"));
    let held = take_where(&places, &ssh, "demo", Lane::ANDROID, Where::Anywhere, 0).unwrap();
    assert_eq!(held.checkout, "m1:app-1");
    held.release();
}

#[test]
fn the_checkout_on_the_less_loaded_host_goes_first() {
    let scratch = Scratch::new("lock-load");
    let places = places_with_host(&scratch, "app-2\nm1:app-1\n");
    // This Mac's load per core is never 1000.
    let busy = ssh_with_load(&scratch.dir.join("busy"), "1000.00");
    let idle = ssh_with_load(&scratch.dir.join("idle"), "0.00");
    let entries = || vec!["app-2".to_string(), "m1:app-1".to_string()];
    let mut loads = Loads {
        places: &places,
        ssh: &busy,
        asked: HashMap::new(),
    };
    let (ordered, skipped) = by_load(&mut loads, entries());
    assert_eq!(ordered, vec!["app-2", "m1:app-1"], "the host is busier");
    assert!(skipped.is_empty());
    let mut loads = Loads {
        places: &places,
        ssh: &idle,
        asked: HashMap::new(),
    };
    let (ordered, _) = by_load(&mut loads, entries());
    assert_eq!(
        ordered,
        vec!["m1:app-1", "app-2"],
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
    let down = crate::testing::ssh_at(&program);
    let Err(error) = take_where(&places, &down, "demo", Lane::ANDROID, Where::Anywhere, 0) else {
        panic!("the only checkout is on a host that is down");
    };
    let text = error.text();
    assert!(
        text.contains("m1:app-1") && text.contains("Connection refused"),
        "{text}"
    );
}

#[test]
fn a_holder_records_how_long_it_waited_and_the_step_it_runs() {
    let scratch = Scratch::new("lock-record");
    let places = places(&scratch);
    let record = places.app_lock_run_file("demo", Lane::MAC, "app-1");
    let first = take(&places, &crate::testing::no_ssh(), "demo", Lane::MAC).unwrap();
    let (said, heard) = channel();
    let (free, freed): (_, Receiver<()>) = channel();
    let waiter = places.clone();
    std::thread::spawn(move || {
        let held = take(&waiter, &crate::testing::no_ssh(), "demo", Lane::MAC).unwrap();
        said.send(()).unwrap();
        freed.recv().unwrap();
        held.runs("macos-tests").unwrap();
        said.send(()).unwrap();
        freed.recv().unwrap();
        held.release();
    });
    until_tickets(&Queue::of(&places, "demo", Lane::MAC), 2);
    std::thread::sleep(SETTLE);
    first.release();
    heard.recv().unwrap();
    let taken = HolderRun::read(&record).unwrap().unwrap();
    assert_eq!(taken.pid, std::process::id() as i32);
    assert!(
        taken.waited_ms >= SETTLE.as_millis() as u64,
        "the second holder waited for the first build: {taken:?}"
    );
    assert_eq!(taken.step, None, "no step runs yet");
    free.send(()).unwrap();
    heard.recv().unwrap();
    let running = HolderRun::read(&record).unwrap().unwrap();
    assert_eq!(
        (running.joined_at, running.waited_ms),
        (taken.joined_at, taken.waited_ms),
        "a step changes neither when it joined nor how long it waited"
    );
    assert_eq!(running.step.unwrap().name, "macos-tests");
    free.send(()).unwrap();
}

/// An ssh that answers every command with a host's probe and <screen>, the
/// lines `host::SCREEN_SCRIPT` prints.
fn ssh_with_screen(dir: &std::path::Path, screen: &str) -> Ssh {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        format!("#!/bin/sh\nprintf 'os=Darwin\\narch=arm64\\ncores=1\\nload=0.00 0 0\\nfree_kib=900000000\\nmemory_free_percent=80\\nswap_used_mib=0\\nmemory_pressure_level=1\\nprocs=300\\nmax_procs=8000\\n{screen}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    crate::testing::ssh_at(&program)
}

#[test]
fn a_build_that_draws_takes_no_host_whose_lid_is_closed_and_says_so() {
    let scratch = Scratch::new("lock-lid");
    let places = places_with_host(&scratch, "m1:app-1\n");
    let closed = ssh_with_screen(&scratch.dir.join("closed"), "lid=closed\\nscreens=0\\n");
    let Err(error) = take_drawing(&places, &closed, "demo", Lane::ANDROID, Where::Anywhere, 0)
    else {
        panic!("the only host cannot draw");
    };
    let text = error.text();
    assert!(
        text.contains("m1:app-1")
            && text.contains("lid is closed")
            && text.contains("cannot draw shots"),
        "{text}"
    );
    // The same host takes a build that draws no scene.
    let held = take_where(&places, &closed, "demo", Lane::ANDROID, Where::Anywhere, 0).unwrap();
    assert_eq!(held.checkout, "m1:app-1");
    held.release();
}

#[test]
fn a_build_that_draws_takes_this_macs_checkout_without_asking_about_a_lid() {
    let scratch = Scratch::new("lock-lid-local");
    let places = places_with_host(&scratch, "app-2\n");
    let closed = ssh_with_screen(&scratch.dir.join("closed"), "lid=closed\\nscreens=0\\n");
    let held = take_drawing(&places, &closed, "demo", Lane::ANDROID, Where::Anywhere, 0).unwrap();
    assert_eq!(held.checkout, "app-2");
    held.release();
}

#[test]
fn a_build_that_draws_takes_a_host_with_its_lid_open_or_another_display() {
    let scratch = Scratch::new("lock-lid-open");
    let places = places_with_host(&scratch, "m1:app-1\n");
    for (name, screen) in [
        ("open", "lid=open\\n"),
        ("external", "lid=closed\\nscreens=1\\n"),
    ] {
        let ssh = ssh_with_screen(&scratch.dir.join(name), screen);
        let held = take_drawing(&places, &ssh, "demo", Lane::ANDROID, Where::Anywhere, 0).unwrap();
        assert_eq!(held.checkout, "m1:app-1", "{name}");
        held.release();
    }
}

#[test]
fn a_command_that_passes_its_limit_leaves_no_git_lock_when_its_git_gets_time_to_remove_it() {
    let scratch = Scratch::new("lock-limit-git");
    let index_lock = scratch.dir.join("index.lock");
    // The step's leader ends on SIGTERM at once. Its child, which holds the
    // lock as git does, removes the lock on SIGTERM after a moment.
    let mut step = Command::new("sh");
    step.args([
        "-c",
        r#"touch "$1"
(trap 'sleep 1; rm -f "$1"; exit 1' TERM; while :; do sleep 0.1; done) &
wait"#,
        "step",
    ])
    .arg(&index_lock);
    let within = run_in_group_within(step, "step", Some(Duration::from_millis(500))).unwrap();
    assert_eq!(within, Within::TimedOut);
    assert!(
        !index_lock.exists(),
        "the lock stayed: git was killed before it removed it"
    );
}

/// Waits for app-1's lock, which this test holds, on a thread of its own,
/// and sends how the wait ended.
fn wait_for_checkout_on_thread(places: &Places) -> Receiver<Result<std::fs::File>> {
    let (said, heard) = channel();
    let places = places.clone();
    std::thread::spawn(move || {
        said.send(take_checkout(&places, "demo", Lane::MAC, "app-1"))
            .unwrap();
    });
    heard
}

#[test]
fn a_wait_for_a_checkouts_lock_ends_with_the_lock_once_its_holder_lets_go() {
    let scratch = Scratch::new("lock-checkout-freed");
    let places = places(&scratch);
    let held = take_checkout(&places, "demo", Lane::MAC, "app-1").unwrap();
    let heard = wait_for_checkout_on_thread(&places);
    assert!(heard.recv_timeout(SETTLE).is_err(), "the lock is held");
    drop(held);
    let taken = heard.recv_timeout(Duration::from_secs(10)).unwrap();
    let file = places.app_lock_file("demo", Lane::MAC, "app-1");
    assert!(crate::flock::names(&taken.unwrap(), &file).unwrap());
}

#[test]
fn a_stop_signal_ends_a_wait_for_a_checkouts_lock() {
    let scratch = Scratch::new("lock-checkout-stopped");
    let places = places(&scratch);
    let _held = take_checkout(&places, "demo", Lane::MAC, "app-1").unwrap();
    end_on_stop_signals();
    let heard = wait_for_checkout_on_thread(&places);
    assert!(heard.recv_timeout(SETTLE).is_err(), "the lock is held");
    // kill() sends the signal to the process, as a supervisor's does.
    assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGTERM) }, 0);
    let ended = heard.recv_timeout(Duration::from_secs(10)).unwrap();
    let said = ended.unwrap_err().text();
    assert!(
        said.contains("stopped on SIGTERM while it waited for app-1's lock"),
        "{said}"
    );
}

/// Sends <signal>, ignored so that no handler runs for it, to this process
/// once a wait's kqueue counts it, and reads the wait's report of it.
fn report_with_no_handler(signal: libc::c_int) {
    unsafe { libc::signal(signal, libc::SIG_IGN) };
    let wake = crate::lock_wake::Wake::new(libc::SIGUSR2).unwrap();
    assert_eq!(unsafe { libc::kill(libc::getpid(), signal) }, 0);
    wake.wait().unwrap();
}

/// The kqueue of a wait reports a signal as soon as it is sent, which can be
/// before the signal's handler runs on the thread the system picked for it.
/// The wait that reads the report records the stop signal.
#[test]
fn a_wait_records_a_stop_signal_before_its_handler_runs() {
    assert_eq!(stop_signal(), None);
    report_with_no_handler(libc::SIGTERM);
    assert_eq!(stop_signal(), Some(libc::SIGTERM));
}

/// `a_wait_records_a_stop_signal_before_its_handler_runs` for a takeover's
/// SIGUSR2, sent to a build that holds its place in a queue.
#[test]
fn a_wait_records_a_takeover_before_its_handler_runs() {
    QUEUED.store(true, Ordering::SeqCst);
    assert!(!takeover_stopped());
    report_with_no_handler(libc::SIGUSR2);
    assert!(takeover_stopped());
}
