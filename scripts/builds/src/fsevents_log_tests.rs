use super::*;
use crate::testing::Scratch;

/// A folder on the Mac's data volume, which records events, is reported as
/// recording them, and the probe leaves no file behind.
#[cfg(target_os = "macos")]
#[test]
fn a_volume_that_records_events_says_so() {
    let scratch = Scratch::new("fsevents-log");
    let places = scratch.places();
    std::fs::create_dir_all(&places.root).unwrap();
    assert!(records_events(&places).unwrap());
    let left: Vec<_> = std::fs::read_dir(places.daemon_dir().join("fsevents-probe"))
        .unwrap()
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn the_health_line_names_each_command_the_person_runs() {
    let scratch = Scratch::new("fsevents-log-line");
    let places = scratch.places();
    let touch = touch_command(&places);
    assert!(touch.starts_with("sudo touch ") && touch.contains("/.fseventsd/no_log"));
    let remount = remount_commands(&places);
    assert!(remount[0].starts_with("diskutil unmount "), "{remount:?}");
    assert!(remount[1].starts_with("diskutil mount "), "{remount:?}");
}
