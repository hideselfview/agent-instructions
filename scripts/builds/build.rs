//! Names the sources this program is built from and keeps a copy of them:
//! `BUILDS_SOURCE` is a hash of every file cargo builds it from, here, in the
//! boards library and in the shots checker beside it; `$OUT_DIR/source.tar`
//! holds those files, which the program carries (`SOURCE`, main.rs). Two
//! builds of the same sources print the same name with `builds version`, on
//! macOS and on Linux, and `builds host setup` sends a Linux host the copy to
//! build there, so the host runs the builds this Mac runs, though each OS
//! builds a program of its own. `BOARDS_SOURCE`, a `CAPTURE_<platform>_SOURCE`
//! for each platform and `COMPARISON_SOURCE` are the versions of the shots
//! checker (`shots_version`): the files that draw a board, those with the
//! files that take the platform's shots, and the files that compare them. They
//! key the board pictures, shots and findings builds stores.

mod build_source;

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn main() {
    let here = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let scripts = here.parent().expect("the crate is in the scripts folder");
    let roots = build_source::roots(&here, scripts);
    for (_, path) in &roots {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let files = build_source::files(&roots);
    let mut hash = Sha256::new();
    let mut tar = Vec::new();
    for (shown, path) in &files {
        let bytes = read(path);
        hash_file(&mut hash, shown, &bytes);
        add_to_tar(&mut tar, shown, &bytes);
    }
    // Two empty blocks end a tar archive.
    tar.extend([0u8; 1024]);
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    std::fs::write(out.join("source.tar"), tar).expect("writing source.tar");
    println!("cargo:rustc-env=BUILDS_SOURCE={}", hex8(hash));
    let lock = here.join("Cargo.lock");
    use build_source::{Side, Taker};
    for (variable, side) in [
        ("BOARDS_SOURCE", Side::Boards),
        ("CAPTURE_MAC_SOURCE", Side::Capture(Taker::Mac)),
        ("CAPTURE_ANDROID_SOURCE", Side::Capture(Taker::Android)),
        ("CAPTURE_WEB_SOURCE", Side::Capture(Taker::Web)),
        ("COMPARISON_SOURCE", Side::Comparison),
    ] {
        println!(
            "cargo:rustc-env={variable}={}",
            shots_version(scripts, &lock, side)
        );
    }
}

/// The version of <side> of the shots checker built from the crates in
/// <scripts> with the Cargo.lock at <lock>: the first 8 bytes, in hex, of a
/// sha256 of what every file that side reads (`build_source::shots_files`)
/// means to the checker (`build_source::meaning`), each by its path and
/// length, and of the lock's entry for each package the checker
/// builds from. A change to builds alone keeps both versions, and a change to
/// the other side keeps this one, so builds keeps what it stored.
fn shots_version(scripts: &Path, lock: &Path, side: build_source::Side) -> String {
    let files = build_source::shots_files(scripts, side);
    let mut hash = Sha256::new();
    for (shown, path) in &files {
        hash_file(&mut hash, shown, &build_source::meaning(shown, read(path)));
    }
    let lock = String::from_utf8(read(lock)).expect("Cargo.lock is UTF-8");
    for entry in lock_entries(&lock, "shots") {
        hash.update(entry.as_bytes());
        hash.update([b'\n']);
    }
    hex8(hash)
}

/// The entries of <lock>, a Cargo.lock, for <root> and each package it
/// builds from, directly or not: each package's name, version, source and
/// checksum on a line, in order.
fn lock_entries(lock: &str, root: &str) -> std::collections::BTreeSet<String> {
    let lock: toml::Table = lock.parse().expect("Cargo.lock is TOML");
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .expect("Cargo.lock lists its packages");
    let field = |package: &toml::Value, key: &str| {
        package
            .get(key)
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut entries = std::collections::BTreeSet::new();
    // Cargo.lock names a dependency by its name, followed by its version and
    // source when the lock holds several packages of that name.
    let mut wanted = vec![root.to_string()];
    while let Some(dependency) = wanted.pop() {
        let mut words = dependency.split(' ');
        let name = words.next().unwrap_or_default();
        let version = words.next();
        let package = packages
            .iter()
            .find(|package| {
                field(package, "name") == name
                    && version.is_none_or(|version| field(package, "version") == version)
            })
            .unwrap_or_else(|| panic!("Cargo.lock has no package {dependency}"));
        let entry = ["name", "version", "source", "checksum"]
            .map(|key| field(package, key))
            .join(" ");
        if !entries.insert(entry) {
            continue;
        }
        // A package with no dependencies has no `dependencies` key.
        let dependencies = package.get("dependencies").map(|list| {
            list.as_array()
                .unwrap_or_else(|| panic!("Cargo.lock lists {name}'s dependencies as no array"))
        });
        for dependency in dependencies.into_iter().flatten() {
            let named = dependency.as_str().unwrap_or_else(|| {
                panic!("Cargo.lock names a dependency of {name} by no text: {dependency}")
            });
            wanted.push(named.to_string());
        }
    }
    entries
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Adds the file <shown>, whose contents are <bytes>, to <hash>.
fn hash_file(hash: &mut Sha256, shown: &str, bytes: &[u8]) {
    hash.update(shown.as_bytes());
    hash.update([0]);
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

/// The first 8 bytes of <hash>'s digest, in hex.
fn hex8(hash: Sha256) -> String {
    hash.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Appends <bytes> to <tar> as the file <name>, in the ustar format: a
/// 512-byte header, then the bytes padded to a multiple of 512. The file's
/// time is 0: the host compares the files by their contents.
fn add_to_tar(tar: &mut Vec<u8>, name: &str, bytes: &[u8]) {
    assert!(
        name.len() <= 100,
        "{name} is longer than a tar header holds"
    );
    let mut header = [0u8; 512];
    let mut put = |at: usize, text: &[u8]| header[at..at + text.len()].copy_from_slice(text);
    put(0, name.as_bytes());
    put(100, b"0000644\0");
    put(108, b"0000000\0");
    put(116, b"0000000\0");
    put(124, format!("{:011o}\0", bytes.len()).as_bytes());
    put(136, b"00000000000\0");
    put(148, b"        ");
    put(156, b"0");
    put(257, b"ustar\0");
    put(263, b"00");
    let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
    header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    tar.extend_from_slice(&header);
    tar.extend_from_slice(bytes);
    tar.resize(tar.len().div_ceil(512) * 512, 0);
}
