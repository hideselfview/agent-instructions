use super::*;

/// Lines of this Mac's power log on 2026-11-20, as `SCRIPT` prints them:
/// the lid closed on battery, then the Mac slept while nobody used it, and
/// m1 slept on power.
const LOG: &str = "2026-11-20 12:51:20 -0500 Sleep               \tEntering Sleep state due to 'Clamshell Sleep':TCPKeepAlive=active Using Batt (Charge:69%) 58 secs   \n\
2026-11-20 12:56:53 -0500 Sleep               \tEntering Sleep state due to 'Idle Sleep':TCPKeepAlive=active Using Batt (Charge:68%) 919 secs  \n\
2026-11-20 11:43:23 -0500 Sleep               \tEntering Sleep state due to 'Clamshell Sleep':TCPKeepAlive=active Using AC (Charge:100%) 7 secs    \n";

fn clamshell() -> Sleep {
    Sleep {
        at: 1_795_197_080,
        seconds: Some(58),
        reason: "Clamshell Sleep".to_string(),
        on_battery: true,
    }
}

#[test]
fn reads_when_each_sleep_began_how_long_it_lasted_and_why() {
    assert_eq!(
        parse(LOG).unwrap(),
        vec![
            clamshell(),
            Sleep {
                at: 1_795_197_413,
                seconds: Some(919),
                reason: "Idle Sleep".to_string(),
                on_battery: true,
            },
            Sleep {
                at: 1_795_193_003,
                seconds: Some(7),
                reason: "Clamshell Sleep".to_string(),
                on_battery: false,
            },
        ]
    );
    assert_eq!(parse("\n").unwrap(), Vec::new());
}

#[test]
fn a_sleep_that_lasts_no_time_yet_has_no_length() {
    let sleeps = parse(
        "2026-11-20 12:51:20 -0500 Sleep  Entering Sleep state due to 'Maintenance Sleep':TCPKeepAlive=active Using AC\n",
    )
    .unwrap();
    assert_eq!(sleeps[0].seconds, None);
    assert!(!sleeps[0].on_battery);
}

#[test]
fn a_line_that_records_no_sleep_fails_naming_it() {
    for line in [
        "2026-11-20 12:52:18 -0500 Wake  Wake from Deep Idle [CDNVA] : due to lid Using BATT (Charge:69%) 275 secs",
        "2026-11-20 12:51:20 Sleep  Entering Sleep state due to 'Idle Sleep' 3 secs",
        "2026-11-20 12:51:20 -05 Sleep  Entering Sleep state due to 'Idle Sleep' 3 secs",
        "2026-11-20 12:51:20 -0500 Sleep  Entering Sleep state",
    ] {
        let error = parse(line).unwrap_err();
        assert!(error.contains(line), "{error}");
    }
}

#[test]
fn a_sleep_counts_when_it_began_or_lasted_into_the_command() {
    let sleep = clamshell();
    let (began, woke) = (sleep.at, sleep.at + 58);
    assert!(sleep.overlaps(began - 100, began));
    assert!(sleep.overlaps(woke, woke + 100));
    assert!(sleep.overlaps(began + 10, began + 20));
    assert!(!sleep.overlaps(woke + 1, woke + 100));
    assert!(!sleep.overlaps(began - 100, began - 1));
    let still_asleep = Sleep {
        seconds: None,
        ..clamshell()
    };
    assert!(still_asleep.overlaps(began + 1000, began + 1100));
    assert!(!still_asleep.overlaps(began - 100, began - 1));
}

#[test]
fn names_each_sleep_and_says_when_neither_mac_slept() {
    let at = local_second(1_795_197_080);
    assert_eq!(
        explain("m1", Ok(vec![clamshell()]), Ok(Vec::new())),
        format!(
            "ssh to host m1 exited 255, as it does when its connection fails or the command exits 255: this Mac's power log shows that it slept at {at} for 58 seconds (Clamshell Sleep, on battery) while the command ran; host m1's power log shows no sleep while the command ran. A Mac that sleeps answers nothing on its connections, so the connection ended"
        )
    );
    assert_eq!(
        explain(
            "m1",
            Ok(Vec::new()),
            Err("sh: pmset: not found".to_string())
        ),
        "ssh to host m1 exited 255, as it does when its connection fails or the command exits 255: this Mac's power log shows no sleep while the command ran; builds could not read host m1's power log: sh: pmset: not found"
    );
}
