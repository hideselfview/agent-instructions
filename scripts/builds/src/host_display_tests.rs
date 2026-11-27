use super::*;
use crate::fail::Fail;
use crate::health::Holder;
use crate::holder_run::{RunTimes, StepTime};
use crate::host::Host;
use crate::host_scan::Scan;
use crate::lane::Lane;
use crate::testing::Scratch;
use std::cell::{Cell, RefCell};

/// What `lsappinfo info -only name,pid "$(lsappinfo front)"` printed on m1
/// while macOS's notification alert held the focus, with the script's
/// newlines turned into spaces.
const ALERT: &str = r#""UserNotificationCenter" ASN:0x0-0x1a01a: (in front)      bundleID=[ NULL ]      bundle path=[ NULL ]      executable path=[ NULL ]      pid = 812 !cgsConnection !signalled type=[ NULL ]  flavor=[ NULL ]  Version=[ NULL ]  Arch=!!none  "#;

/// A reading of a Mac with its lid open, its screen unlocked and Terminal in
/// front, with <change> written over it.
fn reading(change: &[(&str, &str)]) -> String {
    let mut lines = vec![
        ("lid", "open".to_string()),
        ("screen_locked", r#""IOConsoleLocked" = No "#.to_string()),
        (
            "front_app",
            r#""LSDisplayName"="Terminal" "pid"=43839 "#.to_string(),
        ),
    ];
    for (key, value) in change {
        match lines.iter_mut().find(|(known, _)| known == key) {
            Some(line) => line.1 = value.to_string(),
            None => lines.push((key, value.to_string())),
        }
    }
    lines
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect()
}

fn display(change: &[(&str, &str)]) -> Display {
    parse(&reading(change)).unwrap().unwrap()
}

fn host(name: &str) -> Host {
    Host {
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(0.8),
        band: None,
    }
}

fn scan(display: std::result::Result<Option<Display>, String>) -> Scan {
    Scan {
        uid: 501,
        active: Default::default(),
        at: 0,
        rows: Vec::new(),
        cpu_idle: Ok(50.0),
        jobs: Vec::new(),
        sccache: Ok(None),
        display,
    }
}

/// Each reason the person fixes at the Mac holds the step, and each reason
/// in the host's Xcode moves the lane to another host.
#[test]
fn each_reason_has_its_rule() {
    for reason in [
        Reason::ScreenLocked,
        Reason::SystemAlert {
            name: "SecurityAgent".to_string(),
            pid: 90,
        },
        Reason::LidClosed,
        Reason::ChromeCannotStart {
            why: "no address in 120 seconds".to_string(),
        },
    ] {
        assert_eq!(reason.rule(), Rule::Wait, "{reason:?}");
    }
    for reason in [Reason::XcodeCannotStart, Reason::UiAutomationTimesOut] {
        assert_eq!(reason.rule(), Rule::MoveLane, "{reason:?}");
    }
}

/// A reading gives a reason for a locked screen and for a system alert in
/// front, and for a closed lid with no display only to a step that draws
/// window scenes.
#[test]
fn a_reading_gives_each_reason_it_finds() {
    assert_eq!(display(&[]).reasons(true), []);
    assert_eq!(
        display(&[("screen_locked", r#""IOConsoleLocked" = Yes"#)]).reasons(false),
        [Reason::ScreenLocked]
    );
    assert_eq!(
        display(&[("front_app", ALERT)]).reasons(false),
        [Reason::SystemAlert {
            name: "UserNotificationCenter".to_string(),
            pid: 812
        }]
    );
    let closed = display(&[("lid", "closed"), ("screens", "0")]);
    assert_eq!(closed.reasons(true), [Reason::LidClosed]);
    assert_eq!(closed.reasons(false), [], "a step that draws no scene");
    for (lid, screens) in [("closed", "1"), ("none", ""), ("open", "")] {
        let lines: &[(&str, &str)] = &[("lid", lid), ("screens", screens)];
        assert_eq!(display(lines).reasons(true), [], "{lid} {screens}");
    }
}

/// ioreg prints the console's lock as `"IOConsoleLocked" = Yes` and the
/// session's as `"CGSSessionScreenIsLocked"=Yes`; lsappinfo names the front
/// app one of two ways; a host that printed no display line is no Mac, and a
/// line that cannot be read is an error that quotes it.
#[test]
fn reads_the_lid_the_lock_and_the_front_app() {
    assert!(
        display(&[(
            "screen_locked",
            r#""IOConsoleLocked" = No  "CGSSessionScreenIsLocked"=Yes "#
        )])
        .locked
    );
    assert_eq!(
        display(&[("front_app", ALERT)]).front,
        FrontApp {
            name: Some("UserNotificationCenter".to_string()),
            pid: 812
        }
    );
    assert_eq!(
        display(&[(
            "front_app",
            "[ NULL ]  [ NULL ]       bundleID=[ NULL ]      pid = 43839 !cgsConnection "
        )])
        .front,
        FrontApp {
            name: None,
            pid: 43839
        }
    );
    assert_eq!(parse("uid=501\n"), Ok(None));
    for (key, unreadable) in [
        ("screen_locked", "failed: ioreg: no such tool"),
        ("front_app", "lsappinfo: no such app"),
        ("lid", "ajar"),
        ("lid", "failed: ioreg: no such tool"),
    ] {
        let error = parse(&reading(&[(key, unreadable)])).unwrap_err();
        assert!(error.contains(unreadable), "{error}");
    }
    let error = parse(&reading(&[("lid", "closed")])).unwrap_err();
    assert!(error.contains("no screens count"), "{error}");
    let error = parse("lid=open\n").unwrap_err();
    assert!(error.contains("no screen_locked line"), "{error}");
}

/// The script reads the lid from the system's ioreg, the displays from its
/// system_profiler and the front app from its lsappinfo.
#[test]
fn the_script_reads_the_display_with_the_systems_programs() {
    let scratch = Scratch::new("display-script");
    scratch.stand_in(
        "ioreg",
        r#"case "$*" in *AppleClamshellState*) echo '"AppleClamshellState" = Yes';; *) echo '"IOConsoleLocked" = Yes,"other" = 1';; esac"#,
    );
    scratch.stand_in(
        "system_profiler",
        "echo 'Resolution: 1920 x 1080'; echo 'Resolution: 3024 x 1964'",
    );
    scratch.stand_in(
        "lsappinfo",
        r#"case "$1" in front) echo ASN:0x0-0x1a01a;; *) echo '"LSDisplayName"="bae"'; echo '"pid"=93913';; esac"#,
    );
    let read = read(&scratch.system().ssh(), &Host::local()).unwrap();
    assert_eq!(
        read.display,
        Display {
            lid: Lid::Closed { screens: 2 },
            locked: true,
            front: FrontApp {
                name: Some("bae".to_string()),
                pid: 93913
            }
        }
    );
}

/// Each reason a host's reading gives gets a line in the one shape, with
/// what builds does and how to fix it, and a host whose display health could
/// not read gets a line that says why. A closed lid gets none, and neither
/// does a host that is no Mac or whose scan failed.
#[test]
fn each_reason_in_a_hosts_reading_needs_action() {
    let here = Host::local();
    let found = findings(
        &[
            (
                host("m1"),
                Ok(scan(Ok(Some(display(&[("front_app", ALERT)]))))),
            ),
            (
                here,
                Ok(scan(Ok(Some(display(&[(
                    "screen_locked",
                    r#""IOConsoleLocked" = Yes"#,
                )]))))),
            ),
            (
                host("m2"),
                Ok(scan(Ok(Some(display(&[
                    ("lid", "closed"),
                    ("screens", "0"),
                ]))))),
            ),
            (host("m3"), Ok(scan(Ok(None)))),
            (host("m4"), Err("ssh failed".to_string())),
            (
                host("m5"),
                Ok(scan(Err("ioreg named no console lock state".to_string()))),
            ),
        ],
        &[],
    );
    let lines: Vec<String> = found.iter().map(|advice| advice.line()).collect();
    assert_eq!(
        lines,
        [
            "host m1 cannot run steps that open windows: the system alert UserNotificationCenter (pid 812) is in front of every app. builds holds each step that needs windows on host m1 until it ends. Answer or close the alert on host m1, or end it with `ssh u@m1 'kill 812'`",
            "this Mac cannot run steps that open windows: its screen is locked. builds holds each step that needs windows on this Mac until it ends. Unlock the screen on this Mac.",
            "health could not read the display of host m5: ioreg named no console lock state",
        ]
    );
}

/// The lock of <lane>'s <checkout> in repo bae, whose build has waited 7
/// minutes for <wait> to let step <step> start.
fn held_lock(lane: Lane, checkout: &str, step: &str, wait: Option<Wait>) -> LockHealth {
    LockHealth {
        repo: "bae".to_string(),
        lane,
        checkout: checkout.to_string(),
        path: String::new(),
        holder: Some(Holder {
            pid: 4,
            minutes: 12,
            command: "builds app-build".to_string(),
            running: Vec::new(),
            started_from: None,
            app_head: None,
            driver_running: true,
            run: Some(RunTimes {
                moved_at: 0,
                waited_minutes: 0,
                building_minutes: 12,
                step: Some(StepTime {
                    name: step.to_string(),
                    minutes: 7,
                    waits: wait,
                }),
            }),
        }),
    }
}

/// Each reason that holds a step gets a line in the one shape, naming the
/// host, the build, its step and how long the step has waited, the closed
/// lid and Chrome's reason among them. A reason a held step names gets no
/// second line from the host's scan, and a step that runs gets none.
#[test]
fn each_reason_that_holds_a_step_needs_action_with_how_long_it_waited() {
    let alert = Reason::SystemAlert {
        name: "UserNotificationCenter".to_string(),
        pid: 812,
    };
    let found = findings(
        &[(
            host("m1"),
            Ok(scan(Ok(Some(display(&[("front_app", ALERT)]))))),
        )],
        &[
            held_lock(
                Lane::MAC,
                "m2:mac-1",
                "shots",
                Some(Wait::on(&host("m2"), vec![Reason::LidClosed])),
            ),
            held_lock(
                Lane::ANDROID,
                "m1:app-2",
                crate::host_chrome::STEP,
                Some(Wait::on(
                    &host("m1"),
                    vec![Reason::ChromeCannotStart {
                        why: "no address in 120 seconds".to_string(),
                    }],
                )),
            ),
            held_lock(
                Lane::MAC,
                "m1:mac-2",
                "test UITests",
                Some(Wait::on(&host("m1"), vec![alert])),
            ),
            held_lock(Lane::MAC, "m3:mac-3", "macos", None),
        ],
    );
    let lines: Vec<String> = found.iter().map(|advice| advice.line()).collect();
    assert_eq!(
        lines,
        [
            "host m2 cannot run steps that open windows: its lid is closed and it lists no other display; the mac app build of bae has waited 7 minutes on checkout m2:mac-1 to run step shots. builds holds each step that needs windows on host m2 until it ends. Open the lid of host m2, or attach a display to it.",
            "host m1 cannot run steps that open windows: Chrome cannot start there, since the window server does not answer: no address in 120 seconds; the android app build of bae has waited 7 minutes on checkout m1:app-2 to run step start Chrome. builds holds each step that needs windows on host m1 until it ends. The build goes on once the window server on host m1 answers.",
            "host m1 cannot run steps that open windows: the system alert UserNotificationCenter (pid 812) is in front of every app; the mac app build of bae has waited 7 minutes on checkout m1:mac-2 to run step test UITests. builds holds each step that needs windows on host m1 until it ends. Answer or close the alert on host m1, or end it with `ssh u@m1 'kill 812'`",
        ]
    );
}

/// The lane holds the step while the host gives a reason, names each new
/// reason once, blocks on the host between readings, and goes on once the
/// host gives none.
#[test]
fn the_lane_holds_a_step_while_the_host_gives_a_reason_the_person_fixes() {
    let alert = Reason::SystemAlert {
        name: "SecurityAgent".to_string(),
        pid: 90,
    };
    let readings = RefCell::new(
        vec![
            Ok(vec![Reason::ScreenLocked]),
            Ok(vec![Reason::ScreenLocked]),
            Ok(vec![alert.clone()]),
            Ok(vec![Reason::LidClosed]),
            Ok(vec![]),
        ]
        .into_iter(),
    );
    let said = RefCell::new(Vec::new());
    let recorded = RefCell::new(Vec::new());
    let waits = Cell::new(0);
    let ready = wait_until_ready(
        &host("m1"),
        &|| readings.borrow_mut().next().unwrap(),
        &|words| said.borrow_mut().push(words.to_string()),
        &|holds| {
            recorded.borrow_mut().push(holds);
            Ok(())
        },
        &|| {
            waits.set(waits.get() + 1);
            Ok(Ran::Finished)
        },
    )
    .unwrap();
    assert!(ready);
    assert_eq!(waits.get(), 4);
    let recorded = recorded.into_inner();
    let since = recorded[0].as_ref().unwrap().since;
    assert_eq!(
        recorded,
        [
            Some(Hold {
                what: Wait::on(&host("m1"), vec![Reason::ScreenLocked]),
                since
            }),
            Some(Hold {
                what: Wait::on(&host("m1"), vec![alert.clone()]),
                since
            }),
            Some(Hold {
                what: Wait::on(&host("m1"), vec![Reason::LidClosed]),
                since
            }),
            None,
        ],
        "each new reason replaces the hold's record, which keeps when it began"
    );
    assert_eq!(
        said.into_inner(),
        [
            "waiting for host m1 to show windows, since its screen is locked; fix it on that Mac",
            "waiting for host m1 to show windows, since the system alert SecurityAgent (pid 90) is in front of every app; fix it on that Mac",
            "waiting for host m1 to show windows, since its lid is closed and it lists no other display; fix it on that Mac",
            "host m1 shows windows again",
        ]
    );
}

#[test]
fn a_host_that_can_show_windows_holds_nothing() {
    let said = RefCell::new(Vec::new());
    let ready = wait_until_ready(
        &host("m1"),
        &|| Ok(Vec::new()),
        &|words| said.borrow_mut().push(words.to_string()),
        &|_| panic!("no hold"),
        &|| panic!("no wait"),
    )
    .unwrap();
    assert!(ready);
    assert!(said.borrow().is_empty());
}

/// A takeover that stops the build ends the hold, and a host that cannot say
/// fails it.
#[test]
fn a_takeover_ends_the_hold_and_a_failed_reading_fails_it() {
    let locked = || Ok(vec![Reason::ScreenLocked]);
    let m1 = host("m1");
    assert!(!wait_until_ready(&m1, &locked, &|_| {}, &|_| Ok(()), &|| Ok(Ran::Stopped)).unwrap());
    let error = wait_until_ready(
        &m1,
        &|| Err("ssh failed".to_string()),
        &|_| {},
        &|_| Ok(()),
        &|| Ok(Ran::Finished),
    )
    .unwrap_err();
    assert!(error.text().contains("ssh failed"));
}

/// A waiter that a signal ended exits 143. That is no failure of the build:
/// the lane reads the display again, as after a normal return. Any other exit
/// of the waiter fails the build.
#[test]
fn a_waiter_that_a_signal_ended_makes_the_lane_read_the_display_again() {
    let readings = RefCell::new(
        vec![
            Ok(vec![Reason::ScreenLocked]),
            Ok(vec![Reason::ScreenLocked]),
            Ok(vec![]),
        ]
        .into_iter(),
    );
    let waits = RefCell::new(vec![Err(Fail::Exit(143)), Ok(Ran::Finished)].into_iter());
    let said = RefCell::new(Vec::new());
    let ready = wait_until_ready(
        &host("m1"),
        &|| readings.borrow_mut().next().unwrap(),
        &|words| said.borrow_mut().push(words.to_string()),
        &|_| Ok(()),
        &|| waits.borrow_mut().next().unwrap(),
    )
    .unwrap();
    assert!(ready);
    assert_eq!(waits.borrow().len(), 0);
    assert!(
        said.borrow()
            .iter()
            .any(|words| words.contains("host m1") && words.contains("exit 143")),
        "{said:?}"
    );

    let locked = || Ok(vec![Reason::ScreenLocked]);
    let error = wait_until_ready(&host("m1"), &locked, &|_| {}, &|_| Ok(()), &|| {
        Err(Fail::Exit(255))
    })
    .unwrap_err();
    assert!(matches!(error, Fail::Exit(255)));
}

/// A scratch with stand-ins for the programs the wait script runs: `sleep`,
/// on PATH, writes the pid of its first run to the FIFO `sleep.pid` and then
/// sleeps; the stand-in system's `ioreg` reports the screen locked for its
/// first <locked_readings> calls and unlocked after, and its lsappinfo names
/// Terminal.
fn host_programs(scratch: &Scratch, locked_readings: u32) {
    let dir = scratch.dir.as_path();
    crate::testing::fifo(&dir.join("sleep.pid"));
    scratch.script(
        "sleep",
        &format!(
            "[ -e {d}/slept ] || {{ touch {d}/slept; echo $$ > {d}/sleep.pid; }}\nexec /bin/sleep \"$@\"",
            d = dir.display()
        ),
    );
    scratch.stand_in(
        "ioreg",
        &format!(
            "case \"$*\" in *AppleClamshellState*) echo '\"AppleClamshellState\" = No'; exit;; esac\nn=$(cat {d}/readings 2>/dev/null || echo 0)\necho $((n + 1)) > {d}/readings\nif [ \"$n\" -lt {locked_readings} ]; then echo '\"IOConsoleLocked\" = Yes'; else echo '\"IOConsoleLocked\" = No'; fi",
            d = dir.display()
        ),
    );
    scratch.stand_in("lsappinfo", r#"echo '"LSDisplayName"="Terminal" "pid"=1'"#);
}

/// The lane's reading of the scratch's stand-in host (`host_programs`),
/// which says the screen is locked.
fn locked_reading(scratch: &Scratch) -> Reading {
    let reading = read(&scratch.system().ssh(), &Host::local()).unwrap();
    assert!(reading.display.locked);
    reading
}

/// <script> run by the scratch's stand-in system's `sh`, with the scratch's
/// `sleep` first on its path.
fn run_script(scratch: &Scratch, script: &str) -> std::process::Child {
    let path = format!("{}:/usr/bin:/bin", scratch.dir.display());
    scratch
        .system()
        .sh(script)
        .env("PATH", path)
        .spawn()
        .unwrap()
}

/// Waits for the kernel to report that <pid> exited; fails after ten seconds.
fn exits(pid: i32) -> std::io::Result<()> {
    crate::procs::exit_after(pid, || Ok(()))
}

/// The pid that the file <name> of <dir> holds.
fn pid_in(dir: &std::path::Path, name: &str) -> i32 {
    std::fs::read_to_string(dir.join(name))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// The wait reads the display while it waits, so it returns once the
/// reading differs from the lane's, with no notification of any kind.
#[test]
fn the_wait_ends_once_the_reading_changes() {
    let scratch = Scratch::new("display-wait");
    let dir = scratch.dir.as_path();
    host_programs(&scratch, 3);
    let mut waiter = run_script(&scratch, &wait_script(1, &locked_reading(&scratch)));
    pid_in(dir, "sleep.pid");
    let status =
        wait_timeout::ChildExt::wait_timeout(&mut waiter, std::time::Duration::from_secs(30))
            .unwrap();
    let Some(status) = status else {
        waiter.kill().unwrap();
        panic!("the wait never ended");
    };
    assert!(status.success());
}

/// A signal to the script ends it with exit 143 and takes its sleep with it.
#[test]
fn a_signal_to_the_wait_stops_its_sleep() {
    let scratch = Scratch::new("display-wait");
    let dir = scratch.dir.as_path();
    host_programs(&scratch, u32::MAX);
    let mut waiter = run_script(&scratch, &wait_script(1000, &locked_reading(&scratch)));
    let sleeper = pid_in(dir, "sleep.pid");
    unsafe { libc::kill(waiter.id() as i32, libc::SIGTERM) };
    assert_eq!(waiter.wait().unwrap().code(), Some(143));
    exits(sleeper).unwrap();
}

/// A wait whose connection ended, so its parent is gone, ends with its sleep
/// instead of waiting for the display for days.
#[test]
fn a_wait_whose_parent_is_gone_ends_with_its_sleep() {
    let scratch = Scratch::new("display-wait");
    let dir = scratch.dir.as_path();
    host_programs(&scratch, u32::MAX);
    let script_file = dir.join("wait.sh");
    std::fs::write(
        &script_file,
        scratch
            .system()
            .script(&wait_script(3, &locked_reading(&scratch))),
    )
    .unwrap();
    // The outer shell starts the wait, learns from the FIFO that its sleep
    // runs, and exits.
    let outer = format!(
        "sh {f} & echo $! > {d}/waiter.pid; cat {d}/sleep.pid > {d}/sleep.txt",
        f = script_file.display(),
        d = dir.display()
    );
    run_script(&scratch, &outer).wait().unwrap();
    exits(pid_in(dir, "waiter.pid")).unwrap();
    exits(pid_in(dir, "sleep.txt")).unwrap();
}
