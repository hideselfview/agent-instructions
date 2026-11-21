use super::*;
use crate::testing::Scratch;

/// A folder of the Mac's data volume, which keeps a history of its events, is
/// reported as keeping one.
#[cfg(target_os = "macos")]
#[test]
fn a_volume_that_keeps_a_history_says_so() {
    let scratch = Scratch::new("fsevents-log");
    let places = scratch.places();
    std::fs::create_dir_all(&places.root).unwrap();
    assert!(records_events(&places).unwrap());
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
