use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::testing::{Scratch, fill};

const BLOCKED: Duration = Duration::from_millis(400);

fn emptier_in_thread(places: &Places) -> mpsc::Receiver<Result<()>> {
    let (done, finished) = mpsc::channel();
    let places = places.clone();
    std::thread::spawn(move || {
        done.send(empty_now(&places)).unwrap();
    });
    finished
}

fn emptying_folders(places: &Places) -> Vec<PathBuf> {
    std::fs::read_dir(&places.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with(EMPTYING_PREFIX))
        .collect()
}

#[test]
fn an_eviction_holding_the_shared_lock_keeps_an_emptier_from_renaming_the_trash() {
    let scratch = Scratch::new("trash-shared");
    let places = scratch.places();
    fill(&places.trash().join("evict.old/blob"), 4);
    let eviction = lock_shared(&places).unwrap();
    let finished = emptier_in_thread(&places);
    assert!(finished.recv_timeout(BLOCKED).is_err(), "the emptier renamed the trash under an eviction");
    assert!(places.trash().join("evict.old/blob").exists(), "the trash was taken while an eviction held the lock");
    drop(eviction);
    finished.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
    assert!(!places.trash().exists());
    assert!(emptying_folders(&places).is_empty());
}

#[test]
fn an_emptier_never_takes_a_slot_from_an_eviction_that_made_it() {
    let scratch = Scratch::new("trash-slot");
    let places = scratch.places();
    let folder = places.root.join("demo/side-1/target");
    fill(&folder.join("debug/blob"), 4);
    let eviction = lock_shared(&places).unwrap();
    let slot = Slot::make(&places).unwrap();
    let finished = emptier_in_thread(&places);
    assert!(finished.recv_timeout(BLOCKED).is_err());
    // The rename into the slot happens after the emptier asked to take the
    // trash; it must still find the slot.
    std::fs::rename(&folder, slot.path.join("target")).unwrap();
    slot.remove_now().unwrap();
    drop(eviction);
    finished.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
    assert!(!folder.exists() && !places.trash().exists());
}

#[test]
fn an_emptier_waits_for_a_slot_another_process_is_removing() {
    let scratch = Scratch::new("trash-slot-lock");
    let places = scratch.places();
    let eviction = lock_shared(&places).unwrap();
    let slot = Slot::make(&places).unwrap();
    fill(&slot.path.join("blob"), 4);
    drop(eviction);
    let finished = emptier_in_thread(&places);
    assert!(finished.recv_timeout(BLOCKED).is_err(), "the emptier removed a slot another process still holds");
    drop(slot);
    finished.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
    assert!(emptying_folders(&places).is_empty());
}

#[test]
fn an_eviction_after_the_trash_was_taken_makes_a_fresh_one() {
    let scratch = Scratch::new("trash-fresh");
    let places = scratch.places();
    let folder = places.root.join("demo/side-1/target");
    fill(&folder.join("debug/blob"), 4);
    fill(&places.trash().join("evict.old/blob"), 4);
    empty_now(&places).unwrap();
    assert!(crate::evict::evict(&places, &folder, crate::evict::Recreate::No, crate::evict::Removal::Now).unwrap());
    assert!(!folder.exists());
}

#[test]
fn a_folder_a_killed_emptier_left_is_emptied_by_the_next() {
    let scratch = Scratch::new("trash-leftover");
    let places = scratch.places();
    let left = places.root.join(format!("{EMPTYING_PREFIX}ABCDEFGH"));
    fill(&left.join("evict.dead/blob"), 4);
    fill(&places.trash().join("evict.new/blob"), 4);
    empty_now(&places).unwrap();
    assert!(!left.exists() && !places.trash().exists());
}

#[test]
fn a_background_emptier_leaves_a_slot_being_removed_to_its_remover() {
    let scratch = Scratch::new("trash-background");
    let places = scratch.places();
    let _eviction = lock_shared(&places).unwrap();
    let busy = Slot::make(&places).unwrap();
    fill(&busy.path.join("blob"), 4);
    drop(_eviction);
    empty_background(&places).unwrap();
    assert!(!places.trash().exists());
    let moved: Vec<PathBuf> = emptying_folders(&places);
    assert_eq!(moved.len(), 1);
    assert!(moved[0].join(busy.path.file_name().unwrap()).join("blob").exists(), "a slot being removed was started on again");
}
