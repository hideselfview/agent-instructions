use super::*;
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

fn m1() -> Host {
    Host {
        band: None,
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
    }
}

/// The build holds while Chrome prints no address, names the host and
/// records the hold once, tries again after each pause, and goes on once
/// Chrome starts.
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
        &m1(),
        &|| answers.borrow_mut().next().unwrap(),
        &|words| said.borrow_mut().push(words.to_string()),
        &|holds| {
            held.borrow_mut().push(holds.map(|hold| hold.what));
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
        [
            Some(Wait {
                host: "m1".to_string(),
                target: Some("u@m1".to_string()),
                reasons: vec![Reason::ChromeCannotStart {
                    why: "no address in 120 seconds".to_string()
                }],
            }),
            None
        ]
    );
    let said = said.into_inner();
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said[0].contains("waiting for Chrome to start on host m1")
            && said[0].contains("window server does not answer: no address in 120 seconds"),
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
        &m1(),
        &|| Ok(Start::Started),
        &|_| panic!("nothing to say"),
        &|_| panic!("no hold"),
        &|| panic!("no pause"),
    )
    .unwrap();
    assert!(started);
}

#[test]
fn a_takeover_ends_the_hold_and_another_failure_fails_the_build() {
    let hung = || Ok(Start::NoAddress("hung".to_string()));
    let started =
        wait_until_starts(&m1(), &hung, &|_| {}, &|_| Ok(()), &|| Ok(Ran::Stopped)).unwrap();
    assert!(!started);
    let error = wait_until_starts(
        &m1(),
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
        &m1(),
        &|| Err("ssh failed".to_string()),
        &|_| {},
        &|_| Ok(()),
        &|| Ok(Ran::Finished),
    )
    .unwrap_err();
    assert!(error.text().contains("ssh failed"));
}
