use super::*;
use crate::testing::Scratch;

/// A script <name> in <folder> that says it is Python <major>.<minor>.
fn fake(folder: &Path, name: &str, major: u32, minor: u32) -> PathBuf {
    script(folder, name, &format!("echo {major} {minor}"))
}

fn script(folder: &Path, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let file = folder.join(name);
    std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    file
}

fn joined(folders: &[&Path]) -> OsString {
    std::env::join_paths(folders).unwrap()
}

#[test]
fn a_new_enough_python3_first_on_path_leaves_path_as_it_is() {
    let scratch = Scratch::new("python-new");
    let bin = scratch.dir.join("bin");
    fake(&bin, "python3", 3, 14);
    assert_eq!(
        prepare(&joined(&[&bin]), &[], &scratch.dir.join("python")).unwrap(),
        None
    );
}

#[test]
fn python_3_9_first_on_path_gives_way_to_the_newest_python_3_n_of_3_11_or_more() {
    let scratch = Scratch::new("python-old");
    let (system, brew, folder) = (
        scratch.dir.join("system"),
        scratch.dir.join("brew"),
        scratch.dir.join("python"),
    );
    fake(&system, "python3", 3, 9);
    fake(&brew, "python3.10", 3, 10);
    fake(&brew, "python3.12", 3, 12);
    let newest = fake(&brew, "python3.13", 3, 13);
    let path = prepare(&joined(&[&system]), std::slice::from_ref(&brew), &folder)
        .unwrap()
        .unwrap();
    assert_eq!(
        path,
        OsString::from(format!("{}:{}", folder.display(), system.display()))
    );
    assert_eq!(std::fs::read_link(folder.join("python3")).unwrap(), newest);
}

#[test]
fn the_folder_builds_made_is_not_a_candidate_for_its_own_link() {
    let scratch = Scratch::new("python-again");
    let (system, brew, folder) = (
        scratch.dir.join("system"),
        scratch.dir.join("brew"),
        scratch.dir.join("python"),
    );
    fake(&system, "python3", 3, 9);
    let newer = fake(&brew, "python3", 3, 13);
    let first = prepare(&joined(&[&system]), std::slice::from_ref(&brew), &folder)
        .unwrap()
        .unwrap();
    // A child builds started inherits the PATH with the folder first.
    assert_eq!(prepare(&first, &[brew], &folder).unwrap(), None);
    assert_eq!(std::fs::read_link(folder.join("python3")).unwrap(), newer);
}

#[test]
fn with_no_python_of_3_11_or_more_python3_fails_naming_the_version_found() {
    let scratch = Scratch::new("python-none");
    let (system, folder) = (scratch.dir.join("system"), scratch.dir.join("python"));
    fake(&system, "python3", 3, 9);
    let broken = script(&scratch.dir.join("broken"), "python3.12", "exit 3");
    let path = prepare(&joined(&[&system]), &[scratch.dir.join("broken")], &folder)
        .unwrap()
        .unwrap();
    assert!(
        path.to_string_lossy()
            .starts_with(&folder.display().to_string())
    );
    let ran = Command::new(folder.join("python3")).output().unwrap();
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(!ran.status.success());
    assert!(said.contains("Python 3.11 or later"), "{said}");
    assert!(said.contains("system/python3 is Python 3.9"), "{said}");
    assert!(
        said.contains(&format!("{} failed", broken.display())),
        "{said}"
    );
}
