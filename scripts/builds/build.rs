//! Names the sources this program is built from and keeps a copy of them:
//! `BUILDS_SOURCE` is a hash of every file cargo builds it from, here, in the
//! boards library and in the shots checker beside it; `$OUT_DIR/source.tar`
//! holds those files, which the program carries (`SOURCE`, main.rs). Two
//! builds of the same sources print the same name with `builds version`, on
//! macOS and on Linux, and `builds host setup` sends a Linux host the copy to
//! build there, so the host runs the builds this Mac runs, though each OS
//! builds a program of its own. `SHOTS_SOURCE` is the version of the shots
//! checker (`shots_version`), which keys the shot findings builds stores.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The files and folders, relative to a crate's folder, that cargo builds
/// the crate from.
const PARTS: [&str; 5] = ["Cargo.toml", "Cargo.lock", "build.rs", "src", "runtime"];

/// The files and folders, relative to each crate's folder in the scripts
/// folder, that the shots checker is built from, with this crate's
/// Cargo.lock, which pins the checker's dependencies (`shots_version`).
const SHOTS_PARTS: [(&str, &[&str]); 2] = [
    ("shots", &["Cargo.toml", "src", "canvas"]),
    ("boards", &["Cargo.toml", "src", "runtime"]),
];

fn main() {
    let here = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let scripts = here.parent().expect("the crate is in the scripts folder");
    let mut files = Vec::new();
    for name in ["builds", "boards"] {
        for part in PARTS {
            let path = scripts.join(name).join(part);
            println!("cargo:rerun-if-changed={}", path.display());
            collect(&path, &format!("{name}/{part}"), &mut files);
        }
    }
    let (shots, shots_parts) = SHOTS_PARTS[0];
    for part in shots_parts {
        let path = scripts.join(shots).join(part);
        println!("cargo:rerun-if-changed={}", path.display());
        collect(&path, &format!("{shots}/{part}"), &mut files);
    }
    files.sort();
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
    println!(
        "cargo:rustc-env=SHOTS_SOURCE={}",
        shots_version(scripts, &here.join("Cargo.lock"))
    );
}

/// The version of the shots checker built from the crates in <scripts> with
/// the Cargo.lock at <lock>: the first 8 bytes, in hex, of a sha256 of every
/// file of `SHOTS_PARTS`, each by its path and length, and of the lock's
/// entry for each package the checker builds from. A change to builds alone
/// keeps the version, so builds keeps the shot findings it stored.
fn shots_version(scripts: &Path, lock: &Path) -> String {
    let mut files = Vec::new();
    for (name, parts) in SHOTS_PARTS {
        for part in parts {
            collect(
                &scripts.join(name).join(part),
                &format!("{name}/{part}"),
                &mut files,
            );
        }
    }
    files.sort();
    let mut hash = Sha256::new();
    for (shown, path) in &files {
        hash_file(&mut hash, shown, &read(path));
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
        let dependencies = package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten();
        wanted.extend(
            dependencies
                .filter_map(toml::Value::as_str)
                .map(String::from),
        );
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
