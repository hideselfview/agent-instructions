use super::*;
use std::os::unix::fs::PermissionsExt;

/// A folder with a `python3`-named script in <folder> that says it is
/// Python <major>.<minor>.
fn fake(folder: &Path, name: &str, major: u32, minor: u32) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let file = folder.join(name);
    std::fs::write(&file, format!("#!/bin/sh\necho {major} {minor}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    file
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builds-python-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_new_enough_python3_first_on_path_stays_as_it_is() {
    let dir = scratch("new");
    fake(&dir.join("bin"), "python3", 3, 14);
    let path = OsString::from(dir.join("bin"));
    assert_eq!(
        choose(Some(&path), &[], Some(&dir.join("home"))).unwrap(),
        Choice::OnPath
    );
}

#[test]
fn python_3_9_first_on_path_gives_way_to_a_later_python_of_3_11_or_more() {
    let dir = scratch("old");
    fake(&dir.join("system"), "python3", 3, 9);
    let newer = fake(&dir.join("brew"), "python3.13", 3, 13);
    fake(&dir.join("brew"), "python3.10", 3, 10);
    let path = std::env::join_paths([dir.join("system")]).unwrap();
    let home = dir.join("home");
    let Choice::Linked(folder) = choose(Some(&path), &[dir.join("brew")], Some(&home)).unwrap()
    else {
        panic!("the old python3 stayed first");
    };
    assert_eq!(folder, home.join(FOLDER));
    assert_eq!(std::fs::read_link(folder.join("python3")).unwrap(), newer);
    // A second choice that finds another Python replaces the link.
    let newest = fake(&dir.join("brew"), "python3", 3, 14);
    choose(Some(&path), &[dir.join("brew")], Some(&home)).unwrap();
    assert_eq!(std::fs::read_link(folder.join("python3")).unwrap(), newest);
}

#[test]
fn no_python_of_3_11_or_more_fails_naming_the_version_found() {
    let dir = scratch("none");
    fake(&dir.join("system"), "python3", 3, 9);
    let path = OsString::from(dir.join("system"));
    let error = choose(Some(&path), &[dir.join("empty")], Some(&dir.join("home")))
        .unwrap_err()
        .text();
    assert!(error.contains("Python 3.11 or later"), "{error}");
    assert!(error.contains("is Python 3.9"), "{error}");
}

#[test]
fn only_a_python_program_fails_for_want_of_a_new_python() {
    assert!(is_python("python3"));
    assert!(is_python("/usr/bin/python3.9"));
    assert!(!is_python("cargo"));
    assert!(!is_python("scripts/loc-words-test.sh"));
}

#[test]
fn a_linked_python_goes_first_on_the_path_the_step_would_have_had() {
    assert_eq!(
        with_first(Path::new("/h/python"), OsStr::new("/a:/b")),
        OsString::from("/h/python:/a:/b")
    );
    assert_eq!(
        with_first(Path::new("/h/python"), OsStr::new("")),
        OsString::from("/h/python")
    );
}
