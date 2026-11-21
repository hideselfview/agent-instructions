use super::build_source::{files, roots, shots_files};
use crate::testing::Scratch;
use std::path::Path;

/// Writes the file <path>, with its folders, in <dir>.
fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The three crates of a scripts folder, with the builds crate in the folder
/// <builds>.
fn scripts(scratch: &Scratch, builds: &str) -> std::path::PathBuf {
    let scripts = scratch.dir.join("scripts");
    for krate in [builds, "boards", "shots"] {
        write(&scripts, &format!("{krate}/Cargo.toml"), "[package]\n");
        write(&scripts, &format!("{krate}/src/lib.rs"), "// 1\n");
    }
    scripts
}

/// The shown names of the files of <scripts>' builds crate in <builds>.
fn shown(scripts: &Path, builds: &str) -> Vec<String> {
    files(&roots(&scripts.join(builds), scripts))
        .into_iter()
        .map(|(shown, _)| shown)
        .collect()
}

/// install.sh builds the crate from a folder called `src`, not `builds`: the
/// files of the crate are the same ones, under the same names, wherever the
/// folder is, so editing one changes what build.rs hashes.
#[test]
fn the_builds_crate_is_hashed_from_whatever_its_folder_is_called() {
    let scratch = Scratch::new("build-source-folder");
    let in_checkout = scripts(&scratch, "builds");
    let names = shown(&in_checkout, "builds");
    assert!(
        names.contains(&"builds/src/lib.rs".to_string()),
        "{names:?}"
    );
    let scratch = Scratch::new("build-source-folder-src");
    let in_install = scripts(&scratch, "src");
    assert_eq!(shown(&in_install, "src"), names);
    let hashed = |path: &str| {
        files(&roots(&in_install.join("src"), &in_install))
            .into_iter()
            .find(|(shown, _)| shown == path)
            .map(|(_, path)| std::fs::read_to_string(path).unwrap())
    };
    assert_eq!(hashed("builds/src/lib.rs").as_deref(), Some("// 1\n"));
    write(&in_install, "src/src/lib.rs", "// 2\n");
    assert_eq!(hashed("builds/src/lib.rs").as_deref(), Some("// 2\n"));
}

/// A folder that is not the builds crate (no Cargo.toml or no src) makes
/// build.rs fail, so no version hashes only the libraries beside it.
#[test]
#[should_panic(expected = "builds/Cargo.toml")]
fn a_folder_with_no_builds_crate_stops_the_build() {
    let scratch = Scratch::new("build-source-missing");
    let scripts = scripts(&scratch, "builds");
    roots(&scripts.join("nowhere"), &scripts);
}

/// The shots checker's files do not include the builds crate's.
#[test]
fn the_shots_checker_is_hashed_from_its_own_crates() {
    let scratch = Scratch::new("build-source-shots");
    let scripts = scripts(&scratch, "builds");
    let names: Vec<String> = shots_files(&scripts)
        .into_iter()
        .map(|(shown, _)| shown)
        .collect();
    assert_eq!(
        names,
        [
            "boards/Cargo.toml",
            "boards/src/lib.rs",
            "shots/Cargo.toml",
            "shots/src/lib.rs"
        ]
    );
}
