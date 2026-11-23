use super::*;
use crate::health::Holder;
use crate::holder_run::{RunTimes, StepTime};
use crate::lane::Lane;
use std::cell::{Cell, RefCell};

#[test]
fn reads_the_line_builds_shots_chrome_prints() {
    assert_eq!(
        parse("note: x\nchrome=started HeadlessChrome/157.0\n"),
        Ok(Start::Started)
    );
    assert_eq!(
        parse("chrome=no-address cannot start Chrome: no address in 120 seconds"),
        Ok(Start::NoAddress(
            "cannot start Chrome: no address in 120 seconds".to_string()
        ))
    );
    assert_eq!(
        parse("chrome=failed cannot start Chrome: no such file"),
        Ok(Start::Failed(
            "cannot start Chrome: no such file".to_string()
        ))
    );
    assert!(parse("nothing").unwrap_err().contains("no chrome= line"));
    assert!(parse("chrome=maybe").unwrap_err().contains("maybe"));
}

/// The build holds while Chrome prints no address, names the host and the
/// step once, tries again after each pause, and goes on once Chrome starts.
#[test]
fn the_build_holds_a_host_whose_chrome_prints_no_address_until_it_starts() {
    let answers = RefCell::new(
        vec![
            Ok(Start::NoAddress("no address in 120 seconds".to_string())),
            Ok(Start::NoAddress("no address in 120 seconds".to_string())),
            Ok(Start::Started),
        ]
        .into_iter(),
    );
    let said = RefCell::new(Vec::new());
    let held = RefCell::new(Vec::new());
    let pauses = Cell::new(0);
    let started = wait_until_starts(
        "m1",
        &|| answers.borrow_mut().next().unwrap(),
        &|words| said.borrow_mut().push(words.to_string()),
        &|step| {
            held.borrow_mut().push(step.map(str::to_string));
            Ok(())
        },
        &|| {
            pauses.set(pauses.get() + 1);
            Ok(Ran::Finished)
        },
    )
    .unwrap();
    assert!(started);
    assert_eq!(pauses.get(), 2);
    assert_eq!(
        held.into_inner(),
        [Some("waiting for Chrome to start on m1".to_string()), None]
    );
    let said = said.into_inner();
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said[0].contains("waiting for Chrome to start on host m1")
            && said[0].contains("window server does not answer"),
        "{said:?}"
    );
    assert!(
        said[1].contains("Chrome starts on host m1 again"),
        "{said:?}"
    );
}

#[test]
fn a_host_whose_chrome_starts_holds_nothing() {
    let started = wait_until_starts(
        "m1",
        &|| Ok(Start::Started),
        &|_| panic!("nothing to say"),
        &|_| panic!("no step"),
        &|| panic!("no pause"),
    )
    .unwrap();
    assert!(started);
}

#[test]
fn a_takeover_ends_the_hold_and_another_failure_fails_the_build() {
    let hung = || Ok(Start::NoAddress("hung".to_string()));
    let started =
        wait_until_starts("m1", &hung, &|_| {}, &|_| Ok(()), &|| Ok(Ran::Stopped)).unwrap();
    assert!(!started);
    let error = wait_until_starts(
        "m1",
        &|| Ok(Start::Failed("no such file".to_string())),
        &|_| {},
        &|_| Ok(()),
        &|| Ok(Ran::Finished),
    )
    .unwrap_err();
    assert!(
        error
            .text()
            .contains("Chrome cannot start on host m1: no such file")
    );
    let error = wait_until_starts(
        "m1",
        &|| Err("ssh failed".to_string()),
        &|_| {},
        &|_| Ok(()),
        &|| Ok(Ran::Finished),
    )
    .unwrap_err();
    assert!(error.text().contains("ssh failed"));
}

fn lock(checkout: &str, step: Option<&str>) -> LockHealth {
    LockHealth {
        repo: "bae".to_string(),
        lane: Lane::Android,
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
                step: step.map(|name| StepTime {
                    name: name.to_string(),
                    minutes: 7,
                }),
            }),
        }),
    }
}

#[test]
fn health_names_each_host_that_holds_a_build_for_chrome() {
    let found = findings(&[
        lock("m1:app-2", Some("waiting for Chrome to start on m1")),
        lock("m1:app-1", Some("shots android.album.")),
        lock("m1:app-3", None),
    ]);
    let lines: Vec<String> = found.iter().map(|advice| advice.line()).collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("host m1 cannot start Chrome, so the android app build of bae holds checkout m1:app-2 and has built nothing for 7 minutes"),
        "{lines:?}"
    );
}
