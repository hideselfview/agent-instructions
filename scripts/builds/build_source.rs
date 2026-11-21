//! The files cargo builds `builds` from, which build.rs hashes and keeps a
//! copy of, and which build.rs's `cargo:rerun-if-changed` lines name. build.rs
//! includes this file as a module, and the tests in src/build_source_tests.rs
//! include it too.
//!
//! The builds crate's folder is the one cargo builds (`CARGO_MANIFEST_DIR`),
//! whatever it is called: `builds` in a checkout, `src` in the folder that
//! install.sh builds from. The boards and shots crates are the folders of
//! those names beside it.

use std::path::{Path, PathBuf};

/// The files and folders, relative to a crate's folder, that cargo builds
/// the crate from.
const PARTS: [&str; 6] = [
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "build_source.rs",
    "src",
    "runtime",
];

/// The files and folders, relative to each crate's folder in the scripts
/// folder, that the shots checker is built from (`shots_files`).
const SHOTS_PARTS: [(&str, &[&str]); 2] = [
    ("shots", &["Cargo.toml", "src", "canvas"]),
    ("boards", &["Cargo.toml", "src", "runtime"]),
];

/// The roots of the crate <name> in the folder <dir>: each of <parts>, as the
/// name its files carry in the hash and the copy, and its path. Fails when
/// the crate lacks its Cargo.toml or its src: a folder that holds neither is
/// not the crate, and a version that hashes nothing of it names every version
/// alike.
fn crate_roots(name: &str, dir: &Path, parts: &[&str]) -> Vec<(String, PathBuf)> {
    for part in ["Cargo.toml", "src"] {
        assert!(
            dir.join(part).exists(),
            "{} is not there: build.rs hashes {name}/{part} from it",
            dir.join(part).display()
        );
    }
    parts
        .iter()
        .map(|part| (format!("{name}/{part}"), dir.join(part)))
        .collect()
}

/// Each file or folder that builds is built from, as the name its files
/// carry in the hash and the copy, and the path: <here>, the builds crate's
/// folder, and the boards and shots crates in <scripts>.
pub fn roots(here: &Path, scripts: &Path) -> Vec<(String, PathBuf)> {
    let (shots, shots_parts) = SHOTS_PARTS[0];
    let mut roots = crate_roots("builds", here, &PARTS);
    roots.extend(crate_roots("boards", &scripts.join("boards"), &PARTS));
    roots.extend(crate_roots(shots, &scripts.join(shots), shots_parts));
    roots
}

/// Every file under <roots>, sorted: its shown name and its path.
pub fn files(roots: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    for (shown, path) in roots {
        collect(path, shown, &mut files);
    }
    files.sort();
    files
}

/// The side of the shots checker a version covers: the files that take a shot
/// and draw a board, or the files that compare a shot with its board.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// What a cached shot and a cached board picture depend on.
    Capture,
    /// What a cached run's findings depend on, given its shots and pictures.
    Comparison,
}

/// The files of the shots crate's `src` that both sides read: the command
/// line, the shot and element-list types, and the names, settings and
/// declarations that say which shots exist.
const SHARED: [&str; 7] = [
    "lib.rs",
    "shots.rs",
    "names.rs",
    "settings.rs",
    "declared.rs",
    "hex.rs",
    "log.rs",
];

/// The files and folders of the shots crate's `src` that only the capture
/// side reads: taking shots, drawing boards and keeping their pictures.
const CAPTURE_ONLY: [&str; 6] = [
    "capture",
    "render",
    "render.rs",
    "design.rs",
    "board_sizes.rs",
    "pictures.rs",
];

/// The files and folders of the shots crate's `src` that only the comparison
/// side reads: comparing a pair, reading the run it compares with, and writing
/// the findings and the report. `verify.rs` also draws the boards it compares,
/// but the picture of a board is keyed by `design.rs`'s hash of everything it
/// depends on, so what it draws does not depend on `verify.rs`.
const COMPARISON_ONLY: [&str; 10] = [
    "compare",
    "compare.rs",
    "before.rs",
    "findings_file.rs",
    "recheck.rs",
    "report.rs",
    "verify.rs",
    "component_files.rs",
    "lint.rs",
    "lists.rs",
];

/// The files of the shots crate's `src` that neither side reads: `builds shots
/// states`, which lists the component states the boards draw, and `builds
/// shots draws`, which says whether boards draw the same at two commits.
const UNVERSIONED: [&str; 2] = ["states.rs", "draws.rs"];

/// Who reads a file of the shots checker.
#[derive(PartialEq, Eq, Debug)]
enum Readers {
    Both,
    Only(Side),
    Nobody,
}

/// Who reads <shown>, a file of the shots checker (`shots_files`). Fails for
/// a file of the shots crate's `src` that no list above names, so that a new
/// file says which side it belongs to. A test file (`_tests.rs`) has no readers:
/// it changes no run.
fn readers(shown: &str) -> Readers {
    if shown.starts_with("boards/") || shown == "shots/Cargo.toml" {
        return Readers::Both;
    }
    if shown.starts_with("shots/canvas/") {
        return Readers::Only(Side::Capture);
    }
    let Some(inside) = shown.strip_prefix("shots/src/") else {
        panic!("{shown} is no file of the shots checker that build_source.rs knows");
    };
    if inside.ends_with("_tests.rs") {
        return Readers::Nobody;
    }
    let top = inside.split('/').next().unwrap_or(inside);
    if SHARED.contains(&top) {
        Readers::Both
    } else if CAPTURE_ONLY.contains(&top) {
        Readers::Only(Side::Capture)
    } else if COMPARISON_ONLY.contains(&top) {
        Readers::Only(Side::Comparison)
    } else if UNVERSIONED.contains(&top) {
        Readers::Nobody
    } else {
        panic!(
            "{shown} is in no list of build_source.rs (SHARED, CAPTURE_ONLY, COMPARISON_ONLY, UNVERSIONED): say which side of the shots checker reads it"
        )
    }
}

/// Every file of the shots checker that <side> reads, sorted: its shown name
/// and its path, in the crates of <scripts>.
pub fn shots_files(scripts: &Path, side: Side) -> Vec<(String, PathBuf)> {
    let roots: Vec<_> = SHOTS_PARTS
        .iter()
        .flat_map(|(name, parts)| crate_roots(name, &scripts.join(name), parts))
        .collect();
    files(&roots)
        .into_iter()
        .filter(|(shown, _)| match readers(shown) {
            Readers::Both => true,
            Readers::Only(only) => only == side,
            Readers::Nobody => false,
        })
        .collect()
}

/// Adds <path> (shown as <shown>), or every file under it when it is a
/// folder, to <files>. A part a crate lacks adds nothing; any other failure
/// to read a part stops the build.
fn collect(path: &Path, shown: &str, files: &mut Vec<(String, PathBuf)>) {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("reading {}: {error}", path.display()),
    };
    if meta.is_file() {
        files.push((shown.to_string(), path.to_path_buf()));
        return;
    }
    let entries = std::fs::read_dir(path)
        .unwrap_or_else(|error| panic!("listing {}: {error}", path.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("listing {}: {error}", path.display()));
        let name = entry.file_name().to_string_lossy().into_owned();
        collect(&entry.path(), &format!("{shown}/{name}"), files);
    }
}
