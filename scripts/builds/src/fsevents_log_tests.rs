use super::*;
use crate::testing::Scratch;
use std::path::PathBuf;

/// The Mac's system volume is read-only, so no `no_log` file can stop its
/// history of events: a folder of it is reported as keeping one. (The temp
/// folder's volume proves nothing: the person may have stopped its history,
/// which is what `builds health` asks for.)
#[cfg(target_os = "macos")]
#[test]
fn a_volume_that_keeps_a_history_says_so() {
    let scratch = Scratch::new("fsevents-log");
    let places = Places {
        root: PathBuf::from("/"),
        ..scratch.places()
    };
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
