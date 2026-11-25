use super::*;
use crate::fail::Fail;
use crate::host::Host;
use crate::host_scan::Scan;
use std::cell::{Cell, RefCell};

fn host(name: &str) -> Host {
    Host {
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(0.8),
        band: None,
    }
}

fn scan(screen_locked: std::result::Result<Option<bool>, String>) -> Scan {
    Scan {
        uid: 501,
        active: Default::default(),
        at: 0,
        rows: Vec::new(),
        cpu_idle: Ok(50.0),
        jobs: Vec::new(),
        sccache: Ok(None),
        front_app: Ok(None),
        screen_locked,
    }
}

/// ioreg prints the console's lock as `"IOConsoleLocked" = Yes` and the
/// session's as `"CGSSessionScreenIsLocked"=Yes`; a host that printed no line
/// runs no ioreg, and a line with no lock state is an error that quotes it.
#[test]
fn reads_a_locked_screen_from_ioreg() {
    assert_eq!(parse(Some(r#""IOConsoleLocked" = Yes "#)), Ok(Some(true)));
    assert_eq!(
        parse(Some(
            r#""IOConsoleLocked" = No  "CGSSessionScreenIsLocked"=Yes "#
        )),
        Ok(Some(true))
    );
    assert_eq!(parse(Some(r#""IOConsoleLocked" = No "#)), Ok(Some(false)));
    assert_eq!(parse(None), Ok(None));
    for unreadable in ["", "ioreg failed: ioreg: no such tool"] {
        assert!(parse(Some(unreadable)).unwrap_err().contains(unreadable));
    }
}

/// A host whose screen is locked gets a line that names it, and one whose
/// lock health could not read gets a line that says why; one whose screen is
/// not locked, one that runs no ioreg and one whose scan failed get none.
#[test]
fn a_host_with_a_locked_screen_needs_action() {
    let found = findings(&[
        (host("m1"), Ok(scan(Ok(Some(true))))),
        (host("m2"), Ok(scan(Ok(Some(false))))),
        (host("m4"), Ok(scan(Ok(None)))),
        (
            host("m5"),
            Ok(scan(Err("ioreg named no console lock state".to_string()))),
        ),
        (host("m3"), Err("ssh failed".to_string())),
        (Host::local(), Ok(scan(Ok(Some(true))))),
    ]);
    let lines: Vec<String> = found.iter().map(|advice| advice.line()).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines[0].starts_with("host m1 has its screen locked"),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("health could not tell whether the screen is locked on host m5"),
        "{lines:?}"
    );
    assert!(
        lines[2].starts_with("this Mac has its screen locked"),
        "{lines:?}"
    );
}

/// The lane waits while the host reports a locked screen, names the host
/// once, blocks on the host's unlock between readings, and goes on when it
/// unlocks.
#[test]
fn the_lane_holds_a_host_whose_screen_is_locked_until_it_unlocks() {
    let readings = RefCell::new(vec![Ok(true), Ok(true), Ok(false)].into_iter());
    let said = RefCell::new(Vec::new());
    let waits = Cell::new(0);
    let unlocked = wait_until_unlocked(
        "m1",
        &|| readings.borrow_mut().next().unwrap(),
        &|words| said.borrow_mut().push(words.to_string()),
        &|| {
            waits.set(waits.get() + 1);
            Ok(Ran::Finished)
        },
    )
    .unwrap();
    assert!(unlocked);
    assert_eq!(waits.get(), 2);
    let said = said.into_inner();
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said[0].contains("waiting for host m1's screen to unlock"),
        "{said:?}"
    );
    assert!(said[1].contains("host m1's screen is unlocked"), "{said:?}");
}

#[test]
fn an_unlocked_host_waits_for_nothing() {
    let said = RefCell::new(Vec::new());
    let unlocked = wait_until_unlocked(
        "m1",
        &|| Ok(false),
        &|words| said.borrow_mut().push(words.to_string()),
        &|| panic!("no wait"),
    )
    .unwrap();
    assert!(unlocked);
    assert!(said.borrow().is_empty());
}

/// A takeover that stops the build ends the wait, and a host that cannot say
/// fails it.
#[test]
fn a_takeover_ends_the_wait_and_a_failed_reading_fails_it() {
    let unlocked = wait_until_unlocked("m1", &|| Ok(true), &|_| {}, &|| Ok(Ran::Stopped)).unwrap();
    assert!(!unlocked);
    let error = wait_until_unlocked("m1", &|| Err("ssh failed".to_string()), &|_| {}, &|| {
        Ok(Ran::Finished)
    })
    .unwrap_err();
    assert!(error.text().contains("ssh failed"));
}

/// A waiter that a signal ended exits 143. That is no failure of the build:
/// the lane reads the lock again, as after a normal return. Any other exit of
/// the waiter fails the build.
#[test]
fn a_waiter_that_a_signal_ended_makes_the_lane_read_the_lock_again() {
    let readings = RefCell::new(vec![Ok(true), Ok(true), Ok(false)].into_iter());
    let waits = RefCell::new(vec![Err(Fail::Exit(143)), Ok(Ran::Finished)].into_iter());
    let unlocked = wait_until_unlocked(
        "m1",
        &|| readings.borrow_mut().next().unwrap(),
        &|_| {},
        &|| waits.borrow_mut().next().unwrap(),
    )
    .unwrap();
    assert!(unlocked);
    assert_eq!(waits.borrow().len(), 0);

    let error =
        wait_until_unlocked("m1", &|| Ok(true), &|_| {}, &|| Err(Fail::Exit(255))).unwrap_err();
    assert!(matches!(error, Fail::Exit(255)));
}

/// A folder with stand-ins for the programs the wait script runs:
/// `notifyutil` writes its pid to the FIFO `notifyutil.pid` and then waits for
/// a notification that never comes, and `ioreg` reports the screen locked for
/// its first <locked_readings> calls and unlocked after.
fn host_programs(dir: &std::path::Path, locked_readings: u32) {
    crate::testing::fifo(&dir.join("notifyutil.pid"));
    program(
        dir,
        "notifyutil",
        &format!(
            "echo $$ > {}/notifyutil.pid\nexec sleep 1000",
            dir.display()
        ),
    );
    program(
        dir,
        "ioreg",
        &format!(
            "n=$(cat {d}/readings 2>/dev/null || echo 0)\necho $((n + 1)) > {d}/readings\nif [ \"$n\" -lt {locked_readings} ]; then echo '\"IOConsoleLocked\" = Yes'; else echo '\"IOConsoleLocked\" = No'; fi",
            d = dir.display()
        ),
    );
}

fn program(dir: &std::path::Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// <script> run by `sh` with the stand-in programs of <dir> first on its path.
fn run_script(dir: &std::path::Path, script: &str) -> std::process::Child {
    let path = format!("{}:/usr/bin:/bin", dir.display());
    std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("PATH", path)
        .spawn()
        .unwrap()
}

/// Waits for the kernel to report that <pid> exited, for up to ten seconds.
fn exits(pid: i32) -> bool {
    use crate::kqueue::{Queue, event};
    let queue = Queue::new().unwrap();
    if let Err(error) = queue.watch_exit(pid) {
        // The kernel refuses to watch a process that exited and has not been
        // reaped yet, as one the script left to its parent.
        assert_eq!(error.raw_os_error(), Some(libc::ESRCH), "{error}");
        return true;
    }
    let mut notification = [event(0, 0, 0, 0)];
    queue
        .wait(
            &mut notification,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(10)),
        )
        .is_ok()
}

/// The pid that the stand-in `notifyutil` of <dir> wrote, once it started.
fn notifyutil_pid(dir: &std::path::Path) -> i32 {
    std::fs::read_to_string(dir.join("notifyutil.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// The wait reads the lock again while it waits, so it returns once the screen
/// reads unlocked although the notification never comes, and leaves its
/// `notifyutil` behind to no one.
#[test]
fn the_wait_ends_once_the_screen_reads_unlocked_without_the_notification() {
    crate::evict::in_temp_dir(&std::env::temp_dir(), "unlock-wait.", |dir| {
        host_programs(dir, 3);
        let mut waiter = run_script(dir, &wait_script(1));
        let notifyutil = notifyutil_pid(dir);
        let status =
            wait_timeout::ChildExt::wait_timeout(&mut waiter, std::time::Duration::from_secs(30))
                .unwrap();
        let Some(status) = status else {
            waiter.kill().unwrap();
            panic!("the wait never ended");
        };
        assert!(status.success());
        assert!(exits(notifyutil));
        Ok(())
    })
    .unwrap();
}

/// A signal to the script takes its `notifyutil` with it.
#[test]
fn a_signal_to_the_wait_stops_its_notifyutil() {
    crate::evict::in_temp_dir(&std::env::temp_dir(), "unlock-wait.", |dir| {
        host_programs(dir, u32::MAX);
        let mut waiter = run_script(dir, &wait_script(1));
        let notifyutil = notifyutil_pid(dir);
        unsafe { libc::kill(waiter.id() as i32, libc::SIGTERM) };
        assert_eq!(waiter.wait().unwrap().code(), Some(143));
        assert!(exits(notifyutil));
        Ok(())
    })
    .unwrap();
}

/// A listener that fails ends the wait with its exit status, which the lane
/// reports, instead of a wait that looks finished.
#[test]
fn a_listener_that_fails_ends_the_wait_with_its_status() {
    crate::evict::in_temp_dir(&std::env::temp_dir(), "unlock-wait.", |dir| {
        host_programs(dir, u32::MAX);
        program(dir, "notifyutil", "exit 3");
        let mut waiter = run_script(dir, &wait_script(1));
        let status =
            wait_timeout::ChildExt::wait_timeout(&mut waiter, std::time::Duration::from_secs(30))
                .unwrap();
        let Some(status) = status else {
            waiter.kill().unwrap();
            panic!("the wait never ended");
        };
        assert_eq!(status.code(), Some(3));
        Ok(())
    })
    .unwrap();
}

/// A wait whose connection ended, so its parent is gone, ends with its
/// `notifyutil` instead of waiting for an unlock for days.
#[test]
fn a_wait_whose_parent_is_gone_ends_with_its_notifyutil() {
    crate::evict::in_temp_dir(&std::env::temp_dir(), "unlock-wait.", |dir| {
        host_programs(dir, u32::MAX);
        let script_file = dir.join("wait.sh");
        std::fs::write(&script_file, wait_script(1)).unwrap();
        // The outer shell starts the wait, learns from the FIFO that its
        // notifyutil runs, and exits.
        let outer = format!(
            "sh {f} & echo $! > {d}/waiter.pid; cat {d}/notifyutil.pid > {d}/notifyutil.txt",
            f = script_file.display(),
            d = dir.display()
        );
        run_script(dir, &outer).wait().unwrap();
        let pid = |name: &str| -> i32 {
            std::fs::read_to_string(dir.join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        };
        assert!(exits(pid("waiter.pid")));
        assert!(exits(pid("notifyutil.txt")));
        Ok(())
    })
    .unwrap();
}
