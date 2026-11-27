use super::*;
use crate::host::Ssh;
use crate::lane::Lane;
use crate::lock::{Where, take_where};
use crate::testing::Scratch;
use std::sync::mpsc::channel;
use std::time::Duration;

const SETTLE: Duration = Duration::from_millis(300);

/// A program in place of ssh that answers a host probe with no load.
fn idle_ssh(dir: &std::path::Path) -> Ssh {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        "#!/bin/sh\nprintf 'os=Darwin\\narch=arm64\\ncores=1\\nload=0.00 0 0\\nfree_kib=900000000\\nmemory_free_percent=80\\nswap_used_mib=0\\nmemory_pressure_level=1\\nprocs=300\\nmax_procs=8000\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    crate::testing::ssh_at(&program)
}

/// Places whose android lane has <entries> as app checkouts, with the hosts
/// m1 and m2.
fn places_with(scratch: &Scratch, entries: &str) -> (Places, Ssh) {
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::create_dir_all(&places.home).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::ANDROID), entries).unwrap();
    let ssh = idle_ssh(&scratch.dir.join("ssh"));
    crate::host::add(&places, &ssh, "m1", "user@m1").unwrap();
    crate::host::add(&places, &ssh, "m2", "user@m2").unwrap();
    (places, ssh)
}

fn held_on(places: &Places, ssh: &Ssh) -> crate::lock::Held {
    take_where(places, ssh, "demo", Lane::ANDROID, Where::Anywhere, 0).unwrap()
}

#[test]
fn a_second_window_run_on_one_host_waits_for_the_first_and_names_it() {
    let scratch = Scratch::new("window-one-host");
    let (places, ssh) = places_with(&scratch, "m1:app-1\nm1:app-2\n");
    let first_held = held_on(&places, &ssh);
    let second_held = held_on(&places, &ssh);
    assert_eq!(host_of_checkout(&first_held), "m1");
    assert_eq!(host_of_checkout(&second_held), "m1");
    let first = first_held
        .take_window("test one", Access::Exclusive)
        .unwrap()
        .unwrap();
    assert_eq!(
        holder_of(&places, "m1").unwrap().unwrap(),
        format!("demo's {}, step test one", first_held.checkout)
    );

    let (said, heard) = channel();
    let (announced, waiting) = channel();
    let host = host_of_checkout(&second_held).to_string();
    let waiter_places = places.clone();
    let waiter = std::thread::spawn(move || {
        let window = take(
            &waiter_places,
            &host,
            "demo's second, step test two",
            &|words| announced.send(words.to_string()).unwrap(),
            &|| false,
        )
        .unwrap()
        .unwrap();
        said.send(()).unwrap();
        drop(window);
        second_held.release();
    });
    let words = waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("the second run never found the display held");
    assert!(
        words.contains(&format!("demo's {}, step test one", first_held.checkout)),
        "{words}"
    );
    assert!(
        heard.recv_timeout(SETTLE).is_err(),
        "the second run took the display while the first held it"
    );
    drop(first);
    heard.recv_timeout(Duration::from_secs(10)).unwrap();
    waiter.join().unwrap();
    assert_eq!(holder_of(&places, "m1").unwrap(), None);
    first_held.release();
}

#[test]
fn two_shot_captures_share_a_host_and_a_focus_run_waits_for_both() {
    let scratch = Scratch::new("window-shared-display");
    let places = scratch.places();
    let shared = || {
        take_access(
            &places,
            "m1",
            "window shots",
            Access::Shared,
            &|_| (),
            &|| true,
        )
        .unwrap()
        .expect("window shots can join another capture on the shared display")
    };
    let first = shared();
    let second = shared();
    let (waiting, announced) = channel();
    let (acquired, held) = channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _focus = take(
                &places,
                "m1",
                "focus shots",
                &|_| waiting.send(()).unwrap(),
                &|| false,
            )
            .unwrap()
            .unwrap();
            acquired.send(()).unwrap();
        });
        announced.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(first);
        assert!(
            crate::flock::try_lock_creating(&places.app_window_lock_file("m1"), libc::LOCK_EX)
                .unwrap()
                .is_none(),
            "an exclusive run cannot take the display while a shared capture holds it"
        );
        drop(second);
        held.recv_timeout(Duration::from_secs(10)).unwrap();
    });
}

#[test]
fn window_runs_on_two_hosts_run_at_once() {
    let scratch = Scratch::new("window-two-hosts");
    let (places, ssh) = places_with(&scratch, "m1:app-1\nm2:app-1\n");
    let one = held_on(&places, &ssh);
    let two = held_on(&places, &ssh);
    assert_ne!(host_of_checkout(&one), host_of_checkout(&two));
    let first = one
        .take_window("test one", Access::Exclusive)
        .unwrap()
        .unwrap();
    let (said, heard) = channel();
    let other = std::thread::spawn(move || {
        let window = two
            .take_window("test two", Access::Exclusive)
            .unwrap()
            .unwrap();
        said.send(()).unwrap();
        drop(window);
        two.release();
    });
    heard
        .recv_timeout(Duration::from_secs(10))
        .expect("the run on the other host waited");
    other.join().unwrap();
    drop(first);
    one.release();
}

#[test]
fn a_takeover_ends_the_wait_for_the_display() {
    let scratch = Scratch::new("window-takeover");
    let places = scratch.places();
    let held = take(&places, "m1", "test one", &|_| (), &|| false)
        .unwrap()
        .unwrap();
    // The signal reaches the wait's kqueue even when the process ignores it.
    unsafe { libc::signal(libc::SIGUSR2, libc::SIG_IGN) };
    let (announced, waiting) = channel();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter_stop = stop.clone();
    let waiter_places = places.clone();
    let waiter = std::thread::spawn(move || {
        take(
            &waiter_places,
            "m1",
            "test two",
            &|_| announced.send(()).unwrap(),
            &|| waiter_stop.load(std::sync::atomic::Ordering::SeqCst),
        )
        .unwrap()
        .is_none()
    });
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    // What a takeover does: it sets the flag and signals the process. The
    // wait sees it whether it has started to sleep yet or not.
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    unsafe { libc::kill(libc::getpid(), libc::SIGUSR2) };
    assert!(
        waiter.join().unwrap(),
        "the wait went on after the takeover"
    );
    drop(held);
}

fn host_of_checkout(held: &crate::lock::Held) -> &str {
    held.checkout.split_once(':').unwrap().0
}

/// A program in place of ssh that answers a host probe with no load and, to
/// a reading of the display, says the screen is locked for the first
/// <locked> readings and unlocked after them, counting the readings in a
/// file beside it, with the lid open and Terminal in front.
fn locked_ssh(dir: &std::path::Path, locked: u32) -> Ssh {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("ssh");
    let count = dir.join("readings");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n*PPID*) echo x >> '{count}.waits' ;;\n*IOConsoleLocked*)\n  n=$(cat '{count}' 2>/dev/null || echo 0)\n  echo $((n + 1)) > '{count}'\n  echo lid=open\n  echo 'front_app=\"LSDisplayName\"=\"Terminal\" \"pid\"=1'\n  if [ \"$n\" -lt {locked} ]; then echo 'screen_locked=\"IOConsoleLocked\" = Yes'; else echo 'screen_locked=\"IOConsoleLocked\" = No'; fi ;;\n*) printf 'os=Darwin\\narch=arm64\\ncores=1\\nload=0.00 0 0\\nfree_kib=900000000\\nmemory_free_percent=80\\nswap_used_mib=0\\nmemory_pressure_level=1\\nprocs=300\\nmax_procs=8000\\n' ;;\nesac\n",
            count = count.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    crate::testing::ssh_at(&program)
}

#[test]
fn the_mac_lane_holds_a_window_step_while_the_hosts_screen_is_locked() {
    let scratch = Scratch::new("window-locked-screen");
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::create_dir_all(&places.home).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::MAC), "m1:mac-1\n").unwrap();
    let ssh = locked_ssh(&scratch.dir.join("ssh"), 2);
    crate::host::add(&places, &ssh, "m1", "user@m1").unwrap();
    let held = take_where(&places, &ssh, "demo", Lane::MAC, Where::Anywhere, 0).unwrap();
    let window = held
        .take_window("shots", Access::Exclusive)
        .unwrap()
        .expect("the step went on once the screen unlocked");
    let readings: u32 = std::fs::read_to_string(scratch.dir.join("ssh/readings"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(readings, 3, "the lane read the screen until it unlocked");
    let waits = std::fs::read_to_string(scratch.dir.join("ssh/readings.waits")).unwrap();
    assert_eq!(
        waits.lines().count(),
        2,
        "the lane waited on the host's unlock between readings"
    );
    drop(window);
    held.release();
}

/// A program in place of ssh that answers a host probe with no load and, to
/// a reading of the display, says the lid is closed with no display until
/// the file <open> exists, with the screen unlocked and Terminal in front.
/// The wait for the display to change returns once <open> exists.
fn closed_lid_ssh(dir: &std::path::Path, open: &std::path::Path) -> Ssh {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n*PPID*) while [ ! -e '{open}' ]; do sleep 0.05; done ;;\n*AppleClamshellState*)\n  if [ -e '{open}' ]; then echo lid=open; else echo lid=closed; echo screens=0; fi\n  echo 'screen_locked=\"IOConsoleLocked\" = No'\n  echo 'front_app=\"LSDisplayName\"=\"Terminal\" \"pid\"=1' ;;\n*) printf 'os=Darwin\\narch=arm64\\ncores=1\\nload=0.00 0 0\\nfree_kib=900000000\\nmemory_free_percent=80\\nswap_used_mib=0\\nmemory_pressure_level=1\\nprocs=300\\nmax_procs=8000\\n' ;;\nesac\n",
            open = open.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    crate::testing::ssh_at(&program)
}

/// A build that draws window scenes on a host whose lid is closed, when no
/// other host can draw, holds its window step there, and `builds health`
/// names the host, the reason and the held step from the record the build
/// keeps beside its lock. The step goes on once the lid opens.
#[test]
fn health_names_a_window_step_that_a_closed_lid_holds() {
    let scratch = Scratch::new("window-closed-lid");
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::create_dir_all(&places.home).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::MAC), "m1:mac-1\n").unwrap();
    let open = scratch.dir.join("open");
    let ssh = closed_lid_ssh(&scratch.dir.join("ssh"), &open);
    crate::host::add(&places, &ssh, "m1", "user@m1").unwrap();
    let held =
        crate::lock::take_drawing(&places, &ssh, "demo", Lane::MAC, Where::Anywhere, 0).unwrap();
    assert_eq!(held.checkout, "m1:mac-1", "no host left can draw");
    let run_file = places.app_lock_run_file("demo", Lane::MAC, &held.checkout);
    let holds = || {
        crate::holder_run::HolderRun::read(&run_file)
            .unwrap()
            .and_then(|run| run.step)
            .and_then(|step| step.waits)
            .is_some()
    };
    /// Opens the lid when dropped, so that a failed assertion ends the
    /// held step's thread too.
    struct OpensTheLid<'a>(&'a std::path::Path);
    impl Drop for OpensTheLid<'_> {
        fn drop(&mut self) {
            std::fs::write(self.0, "").unwrap();
        }
    }
    std::thread::scope(|scope| {
        let opens = OpensTheLid(&open);
        let step = scope.spawn(|| held.take_window("shots", Access::Exclusive).unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !holds() {
            assert!(
                std::time::Instant::now() < deadline,
                "the lane never recorded the hold"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let run = crate::holder_run::HolderRun::read(&run_file)
            .unwrap()
            .unwrap();
        let lock = crate::health::LockHealth {
            repo: "demo".to_string(),
            lane: Lane::MAC,
            checkout: held.checkout.clone(),
            path: String::new(),
            holder: Some(crate::health::Holder {
                pid: run.pid,
                minutes: 0,
                command: "builds app-build".to_string(),
                running: Vec::new(),
                started_from: None,
                app_head: None,
                driver_running: true,
                run: Some(run.times(crate::app_timing::now())),
            }),
        };
        let lines: Vec<String> = crate::host_display::findings(&[], &[lock])
            .iter()
            .map(|advice| advice.line())
            .collect();
        assert_eq!(
            lines,
            [
                "host m1 cannot run steps that open windows: its lid is closed and it lists no other display; the mac app build of demo has waited 0 minutes on checkout m1:mac-1 to run step shots. builds holds each step that needs windows on host m1 until it ends. Open the lid of host m1, or attach a display to it."
            ]
        );
        drop(opens);
        let window = step
            .join()
            .unwrap()
            .expect("the step went on once the lid opened");
        drop(window);
    });
    assert!(!holds(), "the hold's end cleared it from the record");
    held.release();
}

/// This Mac is a host like any other: the mac lane waits while its own screen
/// is locked.
#[test]
fn the_mac_lane_holds_a_window_step_while_this_macs_screen_is_locked() {
    let scratch = Scratch::new("window-locked-here");
    let places = scratch.places();
    // Locked at the lane's first reading, and unlocked from then on, with
    // the lid open and Terminal in front.
    scratch.stand_in("lsappinfo", r#"echo '"LSDisplayName"="Terminal" "pid"=1'"#);
    scratch.stand_in(
        "ioreg",
        &format!(
            "case \"$*\" in *AppleClamshellState*) echo '\"AppleClamshellState\" = No'; exit;; esac\nif [ -e {read} ]; then echo '\"IOConsoleLocked\" = No'; else echo '\"IOConsoleLocked\" = Yes'; fi\necho x >> {read}",
            read = scratch.dir.join("readings").display()
        ),
    );
    places
        .write_app_checkouts(
            "demo",
            Lane::MAC,
            &[crate::lane::AppCheckout::by_person("app-1").unwrap()],
        )
        .unwrap();
    let held = crate::lock::take(&places, &scratch.system().ssh(), "demo", Lane::MAC).unwrap();
    let window = held
        .take_window("shots", Access::Exclusive)
        .unwrap()
        .expect("the step went on once the screen unlocked");
    let readings = std::fs::read_to_string(scratch.dir.join("readings")).unwrap();
    assert_eq!(
        readings.lines().count(),
        3,
        "the lane's reading, the wait's and the lane's next one"
    );
    drop(window);
    held.release();
}
