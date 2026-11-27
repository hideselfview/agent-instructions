use super::build_source::{Side, Taker, files, roots, shots_files};
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

/// The shown names of the files <side> of the shots checker reads in
/// <scripts>.
fn shots_names(scripts: &Path, side: Side) -> Vec<String> {
    shots_files(scripts, side)
        .into_iter()
        .map(|(shown, _)| shown)
        .collect()
}

/// The shots checker's files do not include the builds crate's.
#[test]
fn the_shots_checker_is_hashed_from_its_own_crates() {
    let scratch = Scratch::new("build-source-shots");
    let scripts = scripts(&scratch, "builds");
    for side in [
        Side::Boards,
        Side::Capture(Taker::Mac),
        Side::Capture(Taker::Android),
        Side::Capture(Taker::Web),
        Side::Comparison,
    ] {
        assert_eq!(
            shots_names(&scripts, side),
            [
                "boards/Cargo.toml",
                "boards/src/lib.rs",
                "shots/Cargo.toml",
                "shots/src/lib.rs"
            ]
        );
    }
}

/// A file of the shots checker is in the capture version or the comparison
/// version by what reads it, so a change to the comparison leaves the capture
/// version, which keys the cached shots and board pictures, as it was. The
/// command line (cli.rs) is in neither version: a new or renamed argument
/// keeps every stored picture and run.
#[test]
fn a_file_only_the_comparison_reads_is_not_in_the_capture_version_and_the_reverse() {
    let scratch = Scratch::new("build-source-sides");
    let scripts = scripts(&scratch, "builds");
    for file in [
        "compare.rs",
        "compare/colour.rs",
        "verify.rs",
        "render.rs",
        "render/board.rs",
        "capture/mod.rs",
        "design.rs",
        "compare_tests.rs",
        "states.rs",
        "cli.rs",
    ] {
        write(&scripts, &format!("shots/src/{file}"), "// 1\n");
    }
    write(&scripts, "shots/canvas/runtime.js", "// 1\n");
    write(&scripts, "shots/harness/mac/ShotScene.swift", "// 1\n");
    write(&scripts, "shots/harness/android/BoardShot.kt", "// 1\n");
    let mac = shots_names(&scripts, Side::Capture(Taker::Mac));
    let android = shots_names(&scripts, Side::Capture(Taker::Android));
    let capture: Vec<String> = mac.iter().chain(&android).cloned().collect();
    let comparison = shots_names(&scripts, Side::Comparison);
    for read in [
        "shots/src/render.rs",
        "shots/src/render/board.rs",
        "shots/src/capture/mod.rs",
        "shots/src/design.rs",
        "shots/canvas/runtime.js",
        "shots/harness/mac/ShotScene.swift",
        "shots/harness/android/BoardShot.kt",
    ] {
        assert!(capture.contains(&read.to_string()), "{read}: {capture:?}");
        assert!(
            !comparison.contains(&read.to_string()),
            "{read}: {comparison:?}"
        );
    }
    for read in [
        "shots/src/compare.rs",
        "shots/src/compare/colour.rs",
        "shots/src/verify.rs",
    ] {
        assert!(
            comparison.contains(&read.to_string()),
            "{read}: {comparison:?}"
        );
        assert!(!capture.contains(&read.to_string()), "{read}: {capture:?}");
    }
    for read in ["shots/src/lib.rs", "boards/src/lib.rs"] {
        assert!(capture.contains(&read.to_string()), "{read}");
        assert!(comparison.contains(&read.to_string()), "{read}");
    }
    for read in [
        "shots/src/compare_tests.rs",
        "shots/src/states.rs",
        "shots/src/cli.rs",
    ] {
        assert!(!capture.contains(&read.to_string()), "{read}");
        assert!(!comparison.contains(&read.to_string()), "{read}");
    }
}

/// A platform's own capture files are in its capture version alone, so a
/// change to one platform's harness keeps the other platforms' runs and every
/// board picture.
#[test]
fn a_platform_s_harness_is_in_no_other_platform_s_capture_version() {
    let scratch = Scratch::new("build-source-takers");
    let scripts = scripts(&scratch, "builds");
    let own = [
        (Taker::Mac, "harness/mac/ShotScene.swift"),
        (Taker::Mac, "harness/mac/display/shot-display.py"),
        (Taker::Mac, "src/harness/mac.rs"),
        (Taker::Android, "harness/android/BoardShot.kt"),
        (Taker::Android, "src/harness/android.rs"),
        (Taker::Web, "src/capture/web.rs"),
    ];
    let shared = ["src/harness.rs", "src/capture/mod.rs", "src/design.rs"];
    for file in own.iter().map(|(_, file)| *file).chain(shared) {
        write(&scripts, &format!("shots/{file}"), "// 1\n");
    }
    for side in [Taker::Mac, Taker::Android, Taker::Web] {
        let names = shots_names(&scripts, Side::Capture(side));
        for (taker, file) in own {
            assert_eq!(
                names.contains(&format!("shots/{file}")),
                taker == side,
                "{side:?} and {file}: {names:?}"
            );
        }
        for file in shared {
            assert!(
                names.contains(&format!("shots/{file}")),
                "{file}: {names:?}"
            );
        }
    }
    let boards = shots_names(&scripts, Side::Boards);
    for (_, file) in own {
        assert!(
            !boards.contains(&format!("shots/{file}")),
            "{file}: {boards:?}"
        );
    }
}

/// A file of the shots crate that no list names makes build.rs fail, so a new
/// file says which side reads it.
#[test]
#[should_panic(expected = "shots/src/brand_new.rs is in no list")]
fn a_shots_file_no_list_names_stops_the_build() {
    let scratch = Scratch::new("build-source-unlisted");
    let scripts = scripts(&scratch, "builds");
    write(&scripts, "shots/src/brand_new.rs", "// 1\n");
    shots_files(&scripts, Side::Boards);
}

/// Every file of the real shots crate is in a list.
#[test]
fn every_file_of_the_shots_crate_is_in_a_list() {
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate is in the scripts folder");
    for side in [
        Side::Boards,
        Side::Capture(Taker::Mac),
        Side::Capture(Taker::Android),
        Side::Capture(Taker::Web),
        Side::Comparison,
    ] {
        assert!(!shots_names(scripts, side).is_empty());
    }
}
