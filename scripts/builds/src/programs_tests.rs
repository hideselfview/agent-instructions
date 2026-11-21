use super::*;
use crate::testing::Scratch;

/// A program at <path> that prints <said> when it is run with `version`.
fn program(path: &Path, said: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!("#!/bin/sh\n[ \"$1\" = version ] && echo '{said}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The programs a build started from, in <scratch>'s folder `bin`: a builds
/// that says `builds <builds>` and a builds-shots that says <shots>.
fn started_from(scratch: &Scratch, builds: &str, shots: &str) -> (PathBuf, PathBuf) {
    let bin = scratch.dir.join("bin");
    let (b, s) = (bin.join("builds"), bin.join("builds-shots"));
    program(&b, &format!("builds {builds}"));
    program(&s, shots);
    (b, s)
}

fn sources<'a>(programs: &'a (PathBuf, PathBuf)) -> Sources<'a> {
    Sources {
        builds: &programs.0,
        shots: &programs.1,
        version: "v1",
        shots_version: "s1",
    }
}

/// Whether some open file holds a lock on the file at <path>.
fn locked(path: &Path) -> bool {
    let file = std::fs::File::open(path).unwrap();
    !crate::flock::try_lock(&file, libc::LOCK_EX, path).unwrap()
}

fn pinned(pin: Pin) -> Pinned {
    match pin {
        Pin::Pinned(pinned) => pinned,
        Pin::Replaced(why) => panic!("expected the programs pinned: {why}"),
    }
}

/// The first build of a version copies the programs it started from into
/// that version's folder and holds the folder while it runs, so no install
/// removes it; PATH then finds that folder's builds first.
#[test]
fn a_build_copies_its_programs_into_its_versions_folder_and_holds_it() {
    let scratch = Scratch::new("programs-pin");
    let home = scratch.dir.join("home");
    let from = started_from(&scratch, "v1", "s1");
    let held = pinned(pin(&home, &sources(&from)).unwrap());
    let dir = folder(&home, "v1");
    assert_eq!(held.builds(), dir.join("builds"));
    for name in ["builds", crate::shots_program::PROGRAM] {
        assert_eq!(
            std::fs::read(dir.join(name)).unwrap(),
            std::fs::read(scratch.dir.join("bin").join(name)).unwrap()
        );
    }
    assert!(locked(&dir.join(IN_USE)), "the build holds its folder");
    assert!(
        held.path(Some(std::ffi::OsStr::new("/usr/bin")))
            .to_string_lossy()
            .starts_with(&format!("{}:", dir.display()))
    );
    drop(held);
    assert!(!locked(&dir.join(IN_USE)));
    // The next build of the version finds the folder, whatever sits where it
    // started from.
    std::fs::remove_dir_all(scratch.dir.join("bin")).unwrap();
    drop(pinned(pin(&home, &sources(&from)).unwrap()));
}

/// A program that says another version was put where this one started
/// from: an install replaced it after this one started, and before this one
/// copied it. The build copies nothing and says so.
#[test]
fn programs_of_another_version_are_not_copied() {
    let scratch = Scratch::new("programs-replaced");
    let home = scratch.dir.join("home");
    for (builds, shots) in [("v2", "s1"), ("v1", "s2")] {
        let from = started_from(&scratch, builds, shots);
        let Pin::Replaced(why) = pin(&home, &sources(&from)).unwrap() else {
            panic!("the programs of another version are not this build's");
        };
        assert!(why.contains("install"), "{why}");
        let dir = folder(&home, "v1");
        assert!(!dir.join("builds").exists() || builds == "v1");
        assert!(!dir.join(crate::shots_program::PROGRAM).exists());
    }
}

/// An install removes each version's folder that no build holds and that it
/// does not keep, a folder a copy has made and not yet put the lock's file in
/// among them.
#[test]
fn an_install_removes_the_folders_no_build_holds_and_it_does_not_keep() {
    let scratch = Scratch::new("programs-remove");
    let home = scratch.dir.join("home");
    for version in ["installed", "named", "held", "old"] {
        let dir = folder(&home, version);
        program(&dir.join("builds"), version);
        drop(crate::flock::open_creating(&dir.join(IN_USE)).unwrap());
    }
    std::fs::create_dir_all(folder(&home, "half-made")).unwrap();
    std::fs::create_dir_all(root(&home).join(".builds.1-0")).unwrap();
    let held_path = folder(&home, "held").join(IN_USE);
    let held = std::fs::File::open(&held_path).unwrap();
    crate::flock::lock(&held, libc::LOCK_SH, &held_path).unwrap();
    assert_eq!(
        versions(&home).unwrap(),
        ["half-made", "held", "installed", "named", "old"]
    );
    let removed = remove_unused(&home, &["installed", "named"]).unwrap();
    assert_eq!(removed, ["half-made", "old"]);
    assert_eq!(versions(&home).unwrap(), ["held", "installed", "named"]);
    assert!(
        remove_unused(&scratch.dir.join("nobody"), &[])
            .unwrap()
            .is_empty()
    );
}

/// `builds programs hold` holds its own version's folder through the exec
/// of its command: the lock's file stays open across exec, and PATH finds
/// the folder's programs first.
#[test]
fn a_hold_keeps_its_lock_across_exec_and_puts_its_folder_first_on_path() {
    let scratch = Scratch::new("programs-hold");
    let home = scratch.dir.join("home");
    let from = started_from(&scratch, "v1", "s1");
    let held = pinned(pin(&home, &sources(&from)).unwrap());
    let (file, path) = held.for_exec(Some(std::ffi::OsStr::new("/bin"))).unwrap();
    let flags = unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(&file), libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, 0, "the lock survives exec");
    assert_eq!(
        path.to_string_lossy(),
        format!("{}:/bin", folder(&home, "v1").display())
    );
}
