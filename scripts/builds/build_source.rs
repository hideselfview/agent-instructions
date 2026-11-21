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
/// folder, that the shots checker is built from, with the builds crate's
/// Cargo.lock, which pins the checker's dependencies (`shots_files`).
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

/// Every file the shots checker is built from, sorted: its shown name and
/// its path, in the crates of <scripts>.
pub fn shots_files(scripts: &Path) -> Vec<(String, PathBuf)> {
    let roots: Vec<_> = SHOTS_PARTS
        .iter()
        .flat_map(|(name, parts)| crate_roots(name, &scripts.join(name), parts))
        .collect();
    files(&roots)
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
