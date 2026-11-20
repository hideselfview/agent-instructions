// The version of builds-shots, shared by the build scripts of builds-shots
// (build.rs here) and of builds (scripts/builds/build.rs), which include this
// file: builds keys the shot findings it stores on the version of the
// builds-shots that install builds beside it from the same sources.

/// The files and folders, relative to each crate's folder in the scripts
/// folder, that cargo builds builds-shots from.
const SHOTS_PARTS: [(&str, &[&str]); 2] = [
    (
        "shots",
        &[
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "version.rs",
            "src",
            "canvas",
        ],
    ),
    ("boards", &["Cargo.toml", "src", "runtime"]),
];

/// The version of the builds-shots built from the crates in <scripts>: the
/// first 8 bytes, in hex, of a sha256 of every file of `SHOTS_PARTS`, each by
/// its path and length. Asks cargo to run the build script again when any of
/// them changes.
fn shots_version(scripts: &std::path::Path) -> String {
    use sha2::Digest;
    let mut files = Vec::new();
    for (name, parts) in SHOTS_PARTS {
        for part in parts {
            let path = scripts.join(name).join(part);
            println!("cargo:rerun-if-changed={}", path.display());
            collect(&path, &format!("{name}/{part}"), &mut files);
        }
    }
    files.sort();
    let mut hash = sha2::Sha256::new();
    for (shown, path) in &files {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        hash.update(shown.as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    hash.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Adds <path> (shown as <shown>), or every file under it when it is a
/// folder, to <files>. A part a crate lacks adds nothing; any other failure
/// to read a part stops the build.
fn collect(path: &std::path::Path, shown: &str, files: &mut Vec<(String, std::path::PathBuf)>) {
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
