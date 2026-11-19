//! Names the sources this program is built from and keeps a copy of them:
//! `BUILDS_SOURCE` is a hash of every file cargo builds it from, here and in
//! the boards library beside it, and `$OUT_DIR/source.tar` holds those files,
//! which the program carries (`SOURCE`, main.rs). Two builds of the same
//! sources print the same name with `builds version`, on macOS and on Linux,
//! and `builds host setup` sends a Linux host the copy to build there, so
//! the host runs the builds this Mac runs, though each OS builds a program of
//! its own.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The files and folders, relative to a crate's folder, that cargo builds
/// the crate from.
const PARTS: [&str; 5] = ["Cargo.toml", "Cargo.lock", "build.rs", "src", "runtime"];

fn main() {
    let here = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let boards = here.join("../boards");
    let mut files = Vec::new();
    for (name, folder) in [("builds", &here), ("boards", &boards)] {
        for part in PARTS {
            let path = folder.join(part);
            println!("cargo:rerun-if-changed={}", path.display());
            collect(&path, &format!("{name}/{part}"), &mut files);
        }
    }
    files.sort();
    let mut hash = Sha256::new();
    let mut tar = Vec::new();
    for (shown, path) in &files {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        hash.update(shown.as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        add_to_tar(&mut tar, shown, &bytes);
    }
    // Two empty blocks end a tar archive.
    tar.extend([0u8; 1024]);
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    std::fs::write(out.join("source.tar"), tar).expect("writing source.tar");
    let digest = hash.finalize();
    let name: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    println!("cargo:rustc-env=BUILDS_SOURCE={name}");
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
