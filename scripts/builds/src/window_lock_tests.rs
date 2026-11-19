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
    Ssh {
        program: program.display().to_string(),
    }
}

/// Places whose android lane has <entries> as app checkouts, with the hosts
/// m1 and m2.
fn places_with(scratch: &Scratch, entries: &str) -> (Places, Ssh) {
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::create_dir_all(&places.home).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::Android), entries).unwrap();
    let ssh = idle_ssh(&scratch.dir.join("ssh"));
    crate::host::add(&places, &ssh, "m1", "user@m1").unwrap();
    crate::host::add(&places, &ssh, "m2", "user@m2").unwrap();
    (places, ssh)
}

fn held_on(places: &Places, ssh: &Ssh) -> crate::lock::Held {
    take_where(places, ssh, "demo", Lane::Android, Where::Hosts, 0).unwrap()
}

#[test]
fn a_second_window_run_on_one_host_waits_for_the_first_and_names_it() {
    let scratch = Scratch::new("window-one-host");
    let (places, ssh) = places_with(&scratch, "m1:app-1\nm1:app-2\n");
    let first_held = held_on(&places, &ssh);
    let second_held = held_on(&places, &ssh);
    assert_eq!(host_of_checkout(&first_held), "m1");
    assert_eq!(host_of_checkout(&second_held), "m1");
    let first = first_held.take_window("test one").unwrap().unwrap();
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
fn window_runs_on_two_hosts_run_at_once() {
    let scratch = Scratch::new("window-two-hosts");
    let (places, ssh) = places_with(&scratch, "m1:app-1\nm2:app-1\n");
    let one = held_on(&places, &ssh);
    let two = held_on(&places, &ssh);
    assert_ne!(host_of_checkout(&one), host_of_checkout(&two));
    let first = one.take_window("test one").unwrap().unwrap();
    let (said, heard) = channel();
    let other = std::thread::spawn(move || {
        let window = two.take_window("test two").unwrap().unwrap();
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

extern "C" fn interrupt(_: libc::c_int) {}

#[test]
fn a_takeover_signal_ends_the_wait_for_the_display() {
    let scratch = Scratch::new("window-takeover");
    let places = scratch.places();
    let held = take(&places, "m1", "test one", &|_| (), &|| false)
        .unwrap()
        .unwrap();
    unsafe {
        let mut action = std::mem::zeroed::<libc::sigaction>();
        action.sa_sigaction = interrupt as *const () as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGUSR2, &action, std::ptr::null_mut());
    }
    let (announced, waiting) = channel();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter_stop = stop.clone();
    let waiter_places = places.clone();
    let waiter = std::thread::spawn(move || {
        let said = take(
            &waiter_places,
            "m1",
            "test two",
            &|_| {
                announced
                    .send(unsafe { libc::pthread_self() as usize })
                    .unwrap()
            },
            &|| waiter_stop.load(std::sync::atomic::Ordering::SeqCst),
        )
        .unwrap();
        said.is_none()
    });
    let thread = waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    // The announcement comes just before the wait starts: signal until the
    // wait ends.
    while !waiter.is_finished() {
        unsafe { libc::pthread_kill(thread as libc::pthread_t, libc::SIGUSR2) };
        std::thread::yield_now();
    }
    assert!(waiter.join().unwrap(), "the wait went on after the signal");
    drop(held);
}

fn host_of_checkout(held: &crate::lock::Held) -> &str {
    held.checkout.split_once(':').unwrap().0
}
