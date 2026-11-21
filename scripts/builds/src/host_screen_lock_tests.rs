use super::*;
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

fn scan(screen_locked: bool) -> Scan {
    Scan {
        uid: 501,
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
/// session's as `"CGSSessionScreenIsLocked"=Yes`; a host that prints neither,
/// or none, is not locked.
#[test]
fn reads_a_locked_screen_from_ioreg() {
    assert!(parse(Some(r#""IOConsoleLocked" = Yes "#)));
    assert!(parse(Some(
        r#""IOConsoleLocked" = No  "CGSSessionScreenIsLocked"=Yes "#
    )));
    assert!(!parse(Some(r#""IOConsoleLocked" = No "#)));
    assert!(!parse(Some("")));
    assert!(!parse(None));
}

/// A host whose screen is locked gets a line that names it; one whose screen
/// is not, and one whose scan failed, get none.
#[test]
fn a_host_with_a_locked_screen_needs_action() {
    let found = findings(&[
        (host("m1"), Ok(scan(true))),
        (host("m2"), Ok(scan(false))),
        (host("m3"), Err("ssh failed".to_string())),
        (Host::local(), Ok(scan(true))),
    ]);
    let lines: Vec<String> = found.iter().map(|advice| advice.line()).collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with("host m1 has its screen locked"),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("this Mac has its screen locked"),
        "{lines:?}"
    );
}

/// The lane waits while the host reports a locked screen, names the host
/// once, and goes on when it unlocks.
#[test]
fn the_lane_holds_a_host_whose_screen_is_locked_until_it_unlocks() {
    let readings = RefCell::new(vec![Ok(true), Ok(true), Ok(false)].into_iter());
    let said = RefCell::new(Vec::new());
    let pauses = Cell::new(0);
    let unlocked = wait_until_unlocked(
        "m1",
        &|| readings.borrow_mut().next().unwrap(),
        &|words| said.borrow_mut().push(words.to_string()),
        &|| false,
        &|| pauses.set(pauses.get() + 1),
    )
    .unwrap();
    assert!(unlocked);
    assert_eq!(pauses.get(), 2);
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
        &|| false,
        &|| panic!("no pause"),
    )
    .unwrap();
    assert!(unlocked);
    assert!(said.borrow().is_empty());
}

/// A takeover that stops the build ends the wait, and a host that cannot say
/// fails it.
#[test]
fn a_takeover_ends_the_wait_and_a_failed_reading_fails_it() {
    let unlocked = wait_until_unlocked("m1", &|| Ok(true), &|_| {}, &|| true, &|| {}).unwrap();
    assert!(!unlocked);
    let error = wait_until_unlocked(
        "m1",
        &|| Err("ssh failed".to_string()),
        &|_| {},
        &|| false,
        &|| {},
    )
    .unwrap_err();
    assert!(error.text().contains("ssh failed"));
}
