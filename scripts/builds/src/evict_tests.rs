use super::*;
use crate::testing::{Scratch, fill};

#[test]
fn a_held_cargo_lock_keeps_the_folder() {
    let scratch = Scratch::new("evict-lock");
    let places = scratch.places();
    let folder = places.root.join("demo/side-1/target");
    fill(&folder.join("debug/blob"), 4);
    std::fs::write(folder.join("debug/.cargo-lock"), "").unwrap();
    // A build in another process holds the lock, as cargo does: a lock this
    // process held would also be held, for a moment, by every child another
    // test forks.
    let lock = folder.join("debug/.cargo-lock");
    let mut build = Command::new("lockf")
        .arg("-k")
        .arg(&lock)
        .args(["sh", "-c", "echo held; exec sleep 60"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(build.stdout.take().unwrap()), &mut said).unwrap();
    assert_eq!(said, "held\n");
    let kept = evict(&places, &folder, Recreate::Yes, Removal::Now);
    build.kill().unwrap();
    build.wait().unwrap();
    assert!(!kept.unwrap(), "a folder whose cargo lock a build holds was deleted");
    assert!(folder.join("debug/blob").exists());
    assert!(evict(&places, &folder, Recreate::Yes, Removal::Now).unwrap());
    assert!(folder.is_dir() && !folder.join("debug").exists(), "the folder was not recreated empty");
}

#[test]
fn a_process_working_inside_keeps_the_folder() {
    let scratch = Scratch::new("evict-cwd");
    let places = scratch.places();
    let folder = places.root.join("DerivedData/entry");
    fill(&folder.join("Build/blob"), 4);
    let mut sleeper = Command::new("sleep").arg("60").current_dir(folder.join("Build")).spawn().unwrap();
    let kept = evict(&places, &folder, Recreate::No, Removal::Now);
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
    assert!(!kept.unwrap(), "a folder a process works in was deleted");
    assert!(evict(&places, &folder, Recreate::No, Removal::Now).unwrap());
    assert!(!folder.exists());
    assert_eq!(std::fs::read_dir(places.trash()).unwrap().count(), 0, "Removal::Now left the slot");
}

#[test]
fn a_build_holding_the_checkouts_guard_keeps_the_folder() {
    // `cargo test` lets go of cargo's lock before it runs the tests it
    // built; `builds cargo` holds the checkout's guard until cargo exits.
    let scratch = Scratch::new("evict-guard");
    let places = scratch.places();
    let root = places.root.join("demo/side-1");
    let folder = root.join("target");
    fill(&folder.join("debug/deps/test-binary"), 4);
    let guard = root.join(crate::units::GUARD);
    std::fs::write(&guard, "").unwrap();
    let mut build = Command::new("lockf")
        .arg("-k")
        .arg(&guard)
        .args(["sh", "-c", "echo held; exec sleep 60"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(build.stdout.take().unwrap()), &mut said).unwrap();
    assert_eq!(said, "held\n");
    let kept = evict(&places, &folder, Recreate::Yes, Removal::Now);
    build.kill().unwrap();
    build.wait().unwrap();
    assert!(!kept.unwrap(), "a folder whose checkout's guard a build holds was deleted");
    assert!(folder.join("debug/deps/test-binary").exists());
    assert!(evict(&places, &folder, Recreate::Yes, Removal::Now).unwrap());
}

#[test]
fn a_folder_another_process_renamed_away_first_is_not_this_eviction_s() {
    // Two processes can pick the same DerivedData entry; the one whose rename
    // finds it gone deleted nothing, and says so.
    let scratch = Scratch::new("evict-lost-rename");
    let places = scratch.places();
    let slot = places.root.join("slot");
    std::fs::create_dir_all(&slot).unwrap();
    let gone = places.root.join("DerivedData/entry");
    let moved = move_in(&gone, &slot, std::ffi::OsStr::new("entry"), Recreate::No).unwrap();
    assert!(!moved, "a rename that found the folder gone counted as a deletion");
    assert!(!gone.exists());
}
