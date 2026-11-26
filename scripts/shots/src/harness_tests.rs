use super::*;
use std::collections::BTreeSet;

/// The names of the files in <folder>, without its subfolders.
fn names(folder: &Path) -> BTreeSet<String> {
    std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

fn carried(files: &[File]) -> BTreeSet<String> {
    files.iter().map(|(name, _)| name.to_string()).collect()
}

#[test]
fn the_program_carries_every_file_of_the_android_harness() {
    let harness = Path::new(env!("CARGO_MANIFEST_DIR")).join("harness/android");
    assert_eq!(carried(ANDROID.bundle), names(&harness));
}

#[test]
fn the_program_carries_every_file_of_the_mac_harness() {
    let harness = Path::new(env!("CARGO_MANIFEST_DIR")).join("harness/mac");
    assert_eq!(carried(MAC.bundle), names(&harness));
    let display = carried(MAC.display.unwrap());
    for name in names(&harness.join("display")) {
        assert!(display.contains(&name), "{name}");
    }
    assert!(display.contains(DISPLAY_TOOL));
}

#[test]
fn a_written_bundle_holds_the_sources_and_keeps_git_out() {
    let scratch = tempfile::tempdir().unwrap();
    let into = scratch.path().join("made/harness");
    MAC.write_bundle(&into).unwrap();
    let mut wanted = carried(MAC.bundle);
    wanted.insert(".gitignore".to_string());
    assert_eq!(names(&into), wanted);
    assert!(
        std::fs::read_to_string(into.join(".gitignore"))
            .unwrap()
            .ends_with("\n*\n")
    );
    // Nothing is left beside the folder.
    assert_eq!(
        std::fs::read_dir(into.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn writing_the_same_sources_again_touches_no_file() {
    let scratch = tempfile::tempdir().unwrap();
    let into = scratch.path().join("harness");
    MAC.write_bundle(&into).unwrap();
    let file = into.join("ShotScene.swift");
    let early = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(early)
        .unwrap();
    MAC.write_bundle(&into).unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), early);
}

#[test]
fn a_folder_an_older_harness_wrote_is_replaced_whole() {
    let scratch = tempfile::tempdir().unwrap();
    let into = scratch.path().join("harness");
    MAC.write_bundle(&into).unwrap();
    std::fs::write(into.join("Removed.swift"), "old").unwrap();
    std::fs::write(into.join("ShotScene.swift"), "old").unwrap();
    MAC.write_bundle(&into).unwrap();
    assert!(!into.join("Removed.swift").exists());
    assert_ne!(
        std::fs::read_to_string(into.join("ShotScene.swift")).unwrap(),
        "old"
    );
}

#[test]
fn a_folder_with_files_of_its_own_is_refused_and_left_as_it_is() {
    let scratch = tempfile::tempdir().unwrap();
    let into = scratch.path().join("sources");
    std::fs::create_dir(&into).unwrap();
    std::fs::write(into.join("Mine.swift"), "mine").unwrap();
    let error = MAC.write_bundle(&into).unwrap_err();
    assert!(matches!(error, HarnessError::NotItsFolder(_)), "{error}");
    assert_eq!(names(&into), BTreeSet::from(["Mine.swift".to_string()]));
}
