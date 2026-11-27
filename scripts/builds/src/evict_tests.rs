use super::*;
use crate::fail::Fail;
use crate::testing::{Scratch, fill};
use std::os::unix::process::CommandExt;

/// Kills <build> and the `sleep` its `lockf` runs: killing `lockf` alone
/// leaves the child alive, holding the test's output open for its 60 seconds.
fn kill_group(build: &mut std::process::Child) {
    let group = build.id() as libc::pid_t;
    assert_eq!(unsafe { libc::kill(-group, libc::SIGKILL) }, 0);
    build.wait().unwrap();
}

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
        .process_group(0)
        .spawn()
        .unwrap();
    let mut said = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(build.stdout.take().unwrap()),
        &mut said,
    )
    .unwrap();
    assert_eq!(said, "held\n");
    let kept = evict(&places, &folder, Recreate::Yes, Removal::Now);
    kill_group(&mut build);
    assert!(
        !kept.unwrap(),
        "a folder whose cargo lock a build holds was deleted"
    );
    assert!(folder.join("debug/blob").exists());
    assert!(evict(&places, &folder, Recreate::Yes, Removal::Now).unwrap());
    assert!(
        folder.is_dir() && !folder.join("debug").exists(),
        "the folder was not recreated empty"
    );
}

#[test]
fn a_process_working_inside_keeps_the_folder() {
    let scratch = Scratch::new("evict-cwd");
    let places = scratch.places();
    let folder = places.root.join("DerivedData/entry");
    fill(&folder.join("Build/blob"), 4);
    let mut sleeper = Command::new("sleep")
        .arg("60")
        .current_dir(folder.join("Build"))
        .spawn()
        .unwrap();
    let kept = evict(&places, &folder, Recreate::No, Removal::Now);
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
    assert!(!kept.unwrap(), "a folder a process works in was deleted");
    assert!(evict(&places, &folder, Recreate::No, Removal::Now).unwrap());
    assert!(!folder.exists());
    assert_eq!(
        std::fs::read_dir(places.trash()).unwrap().count(),
        0,
        "Removal::Now left the slot"
    );
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
        .process_group(0)
        .spawn()
        .unwrap();
    let mut said = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(build.stdout.take().unwrap()),
        &mut said,
    )
    .unwrap();
    assert_eq!(said, "held\n");
    let kept = evict(&places, &folder, Recreate::Yes, Removal::Now);
    kill_group(&mut build);
    assert!(
        !kept.unwrap(),
        "a folder whose checkout's guard a build holds was deleted"
    );
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
    assert!(
        !moved,
        "a rename that found the folder gone counted as a deletion"
    );
    assert!(!gone.exists());
}

/// A checkout's build root as `builds cargo` finds it before its first build:
/// a `.checkout` marker and a target folder, with no guard file.
fn root_without_a_guard(scratch: &Scratch) -> (crate::places::Places, PathBuf, PathBuf) {
    let places = scratch.places();
    let root = places.root.join("demo/side-1");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".checkout"), "/somewhere").unwrap();
    let folder = root.join("target");
    fill(&folder.join("debug/blob"), 4);
    let guard = root.join(crate::units::GUARD);
    assert!(!guard.exists());
    (places, folder, guard)
}

#[test]
fn a_build_root_without_a_guard_yet_gets_one_held_while_the_folder_moves() {
    // `builds cargo` creates the guard on its first build. An eviction before
    // that took no lock, so a build starting between the in-use check and the
    // rename used the folder as it was moved away.
    //
    // The test asserts nothing after the guard is released: another test can
    // fork a child that inherits this process's open files, and the child
    // keeps the lock until it execs, so a later check that the lock is free
    // would depend on timing.
    let scratch = Scratch::new("evict-new-guard");
    let (_places, folder, guard) = root_without_a_guard(&scratch);
    let _held = crate::units::lock_guard_creating(&folder).unwrap();
    assert!(guard.is_file(), "no guard was made");
    assert!(
        crate::evict::try_lock(&guard).unwrap().is_none(),
        "a build could take the guard while the eviction held it"
    );
}

#[test]
fn an_eviction_of_a_build_root_without_a_guard_makes_the_guard_and_moves_the_folder() {
    let scratch = Scratch::new("evict-new-guard-moves");
    let (places, folder, guard) = root_without_a_guard(&scratch);
    assert!(evict(&places, &folder, Recreate::Yes, Removal::Now).unwrap());
    assert!(guard.is_file(), "the eviction made no guard");
    assert!(
        folder.is_dir() && !folder.join("debug").exists(),
        "the folder was not moved and recreated empty"
    );
}

#[test]
fn a_refused_move_returns_the_filesystem_error_and_cleans_the_empty_slot() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("evict-refused-move");
    let places = scratch.places();
    let parent = places.root.join("DerivedData");
    let folder = parent.join("entry");
    fill(&folder.join("Build/blob"), 4);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
    let moved = evict(&places, &folder, Recreate::No, Removal::Now);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(matches!(moved, Err(Fail::Move { from, error, .. })
        if from == folder.canonicalize().unwrap() && error.kind() == std::io::ErrorKind::PermissionDenied));
    assert!(folder.is_dir());
    assert_eq!(std::fs::read_dir(places.trash()).unwrap().count(), 0);
}

#[test]
fn a_removal_that_never_completes_leaves_the_slot_to_the_next_emptier() {
    // A folder the removal cannot empty: its files cannot be unlinked while
    // their directory is read-only. No process holds anything in the folder,
    // so nothing another test forks can make the eviction find it in use.
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("evict-stuck-removal");
    let places = scratch.places();
    let folder = places.root.join("DerivedData/entry");
    fill(&folder.join("Build/blob"), 4);
    let read_only = std::fs::Permissions::from_mode(0o500);
    std::fs::set_permissions(folder.join("Build"), read_only).unwrap();
    let moved = evict(&places, &folder, Recreate::No, Removal::Now);
    let slots: Vec<PathBuf> = std::fs::read_dir(places.trash())
        .unwrap()
        .map(|slot| slot.unwrap().path())
        .collect();
    // Let the emptier below remove it.
    for slot in &slots {
        std::fs::set_permissions(
            slot.join("entry/Build"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    assert!(
        moved.expect("a removal that could not finish failed the build"),
        "the folder had been moved"
    );
    assert!(!folder.exists());
    assert_eq!(slots.len(), 1, "the slot is the trash's to empty");
    crate::trash::empty_now(&places).unwrap();
    assert_eq!(
        std::fs::read_dir(places.trash()).map_or(0, |dir| dir.count()),
        0
    );
}
