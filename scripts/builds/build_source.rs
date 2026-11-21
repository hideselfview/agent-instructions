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

/// Each file or folder that builds is built from, as the name its files
/// carry in the hash and the copy, and the path: <here>, the builds crate's
/// folder, and the boards and shots crates in <scripts>. Fails when a crate
/// lacks its Cargo.toml or its src: a folder that holds neither is not the
/// crate, and a version that hashes nothing of it names every version alike.
pub fn roots(here: &Path, scripts: &Path) -> Vec<(String, PathBuf)> {
    let mut roots = Vec::new();
    for (name, dir) in [
        ("builds", here.to_path_buf()),
        ("boards", scripts.join("boards")),
    ] {
        for part in PARTS {
            roots.push((format!("{name}/{part}"), dir.join(part)));
        }
    }
    let (shots, shots_parts) = SHOTS_PARTS[0];
    for part in shots_parts {
        roots.push((format!("{shots}/{part}"), scripts.join(shots).join(part)));
    }
    for crate_name in ["builds", "boards", "shots"] {
        for part in ["Cargo.toml", "src"] {
            let shown = format!("{crate_name}/{part}");
            let (_, path) = roots
                .iter()
                .find(|(name, _)| *name == shown)
                .expect("every crate lists its Cargo.toml and src");
            assert!(
                path.exists(),
                "{} is not there: build.rs hashes {shown} from it",
                path.display()
            );
        }
    }
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
    let mut roots = Vec::new();
    for (name, parts) in SHOTS_PARTS {
        for part in parts {
            roots.push((format!("{name}/{part}"), scripts.join(name).join(part)));
        }
    }
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
