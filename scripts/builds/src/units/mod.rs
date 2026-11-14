//! Cargo's build copies in a build folder: recording which ones a build used,
//! and deleting the ones no build used for a while, the ones a newer copy of
//! the same thing replaced, and the object files no build or debugger reads.
//!
//! Cargo builds each crate target as a unit and names the unit's files after
//! a hash of everything that build depends on, dependency versions included,
//! so a dependency bump builds a new copy of every unit above it and the old
//! copies stay. In each profile folder (<target>/debug,
//! <target>/<triple>/release, and a nested target such as a `cargo install
//! --target-dir` one) a unit owns
//!
//!     .fingerprint/<package>-<hash>/   cargo's record that the unit is built
//!     deps/<crate>-<hash>*, deps/lib<crate>-<hash>*, examples/<name>-<hash>*
//!     build/<package>-<hash>/          a build script, or what running it wrote
//!     incremental/<crate>-<id>/        rustc's incremental state
//!
//! A cdylib or staticlib unit's files in deps carry no hash: each build of
//! that crate overwrites them, so they never pile up and nothing here deletes
//! them. The incremental folder's id is rustc's, not cargo's hash; it belongs
//! to the unit whose deps files are hard links to its codegen and metadata
//! files.
//!
//! A unit's files are deleted only while this holds cargo's lock on the
//! profile folder and the checkout's `GUARD`, which `builds cargo` holds for
//! a build's whole life, and never a file a process holds open. Cargo
//! decides which units are up to date for its whole unit graph before it
//! runs any job, and nothing outside cargo can see that graph, so no unit is
//! deleted while a build runs: a build that starts after a deletion finds
//! the unit's fingerprint gone and builds it again. `builds cargo` starts
//! `units after-build` as it exits, so a copy another replaced goes moments
//! after the build that replaced it.
//!
//! A build that finds a unit up to date writes nothing in any of these, so
//! its use is recorded from cargo's own report (`--message-format=json-...`):
//! `record` sets the modification time of the unit's fingerprint folder to
//! the moment that build's cargo began reporting, one time for every unit of
//! the build. A unit's last use is the newest modification time of that
//! folder and the files in it, which for a unit no recorded build has used is
//! when cargo last built it.

pub mod git_checkouts;
pub mod idle;
pub mod objects;
pub mod prune;
pub mod record;

use std::collections::HashSet;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};

/// Folders a profile folder keeps per-unit files in, beside .fingerprint.
pub const OUTPUT_DIRS: [&str; 2] = ["deps", "examples"];
/// How deep below a build folder a profile folder can sit:
/// owner-construction-check/<rev>/release.
const PROFILE_DEPTH: usize = 3;
/// How deep inside an incremental folder a write counts toward its last one:
/// <crate>-<id>/s-<session>-<hash>/ and the files in it are three levels.
pub const INCREMENTAL_DEPTH: usize = 8;

/// The crate and hash a unit's output file names:
/// deps/libbae_core-8fffeaeaf85dc425.rlib, deps/bae_core-8fffeaeaf85dc425 (a
/// test), deps/bae_core-8fffeaeaf85dc425.bae_core.1c1aebe005fd4a7f-cgu.00.rcgu.o.
pub fn hashed_file(name: &str) -> Option<(&str, &str)> {
    fn parse(rest: &str) -> Option<(&str, &str)> {
        let dash = rest.find('-')?;
        let krate = &rest[..dash];
        if krate.is_empty()
            || !krate
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return None;
        }
        let after = &rest[dash + 1..];
        let hash = after.get(..16)?;
        if !is_hash(hash) {
            return None;
        }
        let tail = &after[16..];
        if tail.is_empty() || (tail.starts_with('.') && tail.len() > 1) {
            Some((krate, hash))
        } else {
            None
        }
    }
    name.strip_prefix("lib")
        .and_then(parse)
        .or_else(|| parse(name))
}

/// The package and hash of a unit's folder:
/// .fingerprint/bae-core-8fffeaeaf85dc425, build/aws-lc-sys-3de0343d4993181b.
pub fn unit_dir(name: &str) -> Option<(&str, &str)> {
    if name.len() < 18 || !name.is_char_boundary(name.len() - 17) {
        return None;
    }
    let (package, rest) = name.split_at(name.len() - 17);
    let hash = rest.strip_prefix('-')?;
    if is_hash(hash) {
        Some((package, hash))
    } else {
        None
    }
}

fn is_hash(text: &str) -> bool {
    text.len() == 16
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn is_profile_dir(path: &Path) -> bool {
    path.join(".fingerprint").is_dir()
}

/// Every profile folder in <folder>, at most PROFILE_DEPTH levels down.
pub fn profile_dirs(folder: &Path) -> Vec<PathBuf> {
    fn walk(path: &Path, depth: usize, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
        if entries.iter().any(|entry| {
            entry.file_name() == ".fingerprint" && entry.file_type().is_ok_and(|kind| kind.is_dir())
        }) {
            found.push(path.to_path_buf());
            return;
        }
        if depth == PROFILE_DEPTH {
            return;
        }
        for entry in entries {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) && entry.file_name() != "doc" {
                walk(&entry.path(), depth + 1, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(folder, 0, &mut found);
    found
}

/// File names in <dir> as text (non-UTF-8 names name no unit).
pub fn names_in(dir: &Path) -> Result<Vec<String>> {
    let entries = std::fs::read_dir(dir).context(|| format!("listing {}", dir.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", dir.display()))?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Seconds since 1970, with the fraction.
pub fn mtime_of(meta: &std::fs::Metadata) -> f64 {
    meta.mtime() as f64 + meta.mtime_nsec() as f64 / 1e9
}

/// The newest modification time of <path> and what it holds, <depth> levels
/// down; 0 when it is missing.
pub fn newest_mtime(path: &Path, depth: usize) -> Result<f64> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0.0),
        Err(error) => fail!("reading {}: {error}", path.display()),
    };
    let mut newest = mtime_of(&meta);
    if depth > 0 && meta.is_dir() {
        let entries = match std::fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(newest),
            Err(error) => fail!("listing {}: {error}", path.display()),
        };
        for entry in entries {
            let entry = entry.context(|| format!("listing {}", path.display()))?;
            newest = newest.max(newest_mtime(&entry.path(), depth - 1)?);
        }
    }
    Ok(newest)
}

/// Takes cargo's lock on <profile> without waiting, as cargo holds it while
/// it builds: the open lock, or None while a build holds it.
pub fn lock_profile(profile: &Path) -> Result<Option<File>> {
    crate::evict::try_lock_creating(&profile.join(".cargo-lock"))
}

/// The file in a checkout's build root that `builds cargo` holds a shared
/// lock on from before it starts cargo until cargo has exited and every unit
/// cargo reported is recorded. Cargo's own lock doesn't cover a build's
/// whole use of its folder: `cargo test` releases it once it has compiled,
/// then runs each test binary and rustdoc on the doctests, which reads the
/// rlibs; and the recorder may still be reading cargo's report after cargo
/// let go. Deleting build copies, or a whole folder, takes it exclusively
/// without waiting, beside cargo's.
pub const GUARD: &str = ".builds-cargo.lock";

/// The guard of the checkout <path> is in: the nearest <GUARD> at <path> or
/// above it.
pub fn guard_of(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .map(|dir| dir.join(GUARD))
        .find(|guard| guard.is_file())
}

/// Whether a `builds cargo` build is running in the checkout a path is in.
pub enum Guard {
    /// No guard is above the path: no `builds cargo` build takes one there.
    Absent,
    /// Held exclusively by this process until dropped; the file is kept
    /// only to hold the lock.
    Held(#[allow(dead_code)] File),
    /// A `builds cargo` build holds it.
    Busy,
}

/// Takes the guard above <path> exclusively, without waiting.
pub fn lock_guard(path: &Path) -> Result<Guard> {
    let Some(guard) = guard_of(path) else {
        return Ok(Guard::Absent);
    };
    match crate::evict::try_lock(&guard) {
        Ok(Some(file)) => Ok(Guard::Held(file)),
        Ok(None) => Ok(Guard::Busy),
        // Deleted since it was found: its checkout's root is being deleted.
        Err(_) if !guard.exists() => Ok(Guard::Absent),
        Err(error) => Err(error),
    }
}

/// Like `lock_guard`, for a folder in a checkout's build root (the nearest
/// folder at or above <path> with a `.checkout` marker): creates the guard
/// when it does not exist yet, which is when no `builds cargo` build has run
/// there. A build that starts after this takes the guard, as `builds cargo`
/// does, waits for it, so no build can begin using <path> between its being
/// judged idle and its move. A path in no build root has no guard.
pub fn lock_guard_creating(path: &Path) -> Result<Guard> {
    let Some(root) = path.ancestors().find(|dir| dir.join(".checkout").is_file()) else {
        return lock_guard(path);
    };
    match crate::evict::try_lock_creating(&root.join(GUARD))? {
        Some(file) => Ok(Guard::Held(file)),
        None => Ok(Guard::Busy),
    }
}

/// The files and folders under <roots> that a process other than this one
/// holds open (a file, a working directory, a running executable, a mapped
/// file), by inode: lsof names a hard-linked file by whichever of its names
/// it finds, and rustc hard-links a unit's deps files into its incremental
/// folder. Listed once, when first asked, which is after the folder's locks
/// are held.
pub struct OpenFiles {
    roots: Vec<Vec<u8>>,
    inodes: Option<HashSet<u64>>,
}

impl OpenFiles {
    pub fn new(roots: &[PathBuf]) -> OpenFiles {
        OpenFiles {
            roots: roots
                .iter()
                .map(|root| {
                    let mut bytes = root.as_os_str().as_bytes().to_vec();
                    bytes.push(b'/');
                    bytes
                })
                .collect(),
            inodes: None,
        }
    }

    pub fn get(&mut self) -> Result<&HashSet<u64>> {
        if self.inodes.is_none() {
            let me = std::process::id().to_string();
            let output = Command::new("lsof")
                .args(["-n", "-Fpin"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .context(|| "running lsof".to_string())?;
            let mut inodes = HashSet::new();
            let mut pid: &[u8] = b"";
            let mut inode: Option<u64> = None;
            for line in output.stdout.split(|byte| *byte == b'\n') {
                match line.first() {
                    Some(b'p') => pid = &line[1..],
                    Some(b'f') => inode = None,
                    Some(b'i') => {
                        inode = std::str::from_utf8(&line[1..])
                            .ok()
                            .and_then(|text| text.parse().ok())
                    }
                    Some(b'n') if pid != me.as_bytes() => {
                        let name = &line[1..];
                        if let Some(inode) = inode
                            && self.roots.iter().any(|root| name.starts_with(root))
                        {
                            inodes.insert(inode);
                        }
                    }
                    _ => {}
                }
            }
            self.inodes = Some(inodes);
        }
        Ok(self.inodes.as_ref().expect("listed above"))
    }
}

/// Does a process hold any of <paths>, or a file inside one, open?
pub fn holds_any(paths: &[PathBuf], inodes: &HashSet<u64>) -> Result<bool> {
    if inodes.is_empty() {
        return Ok(false);
    }
    for path in paths {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => fail!("reading {}: {error}", path.display()),
        };
        if inodes.contains(&meta.ino()) {
            return Ok(true);
        }
        if meta.is_dir() {
            let found = std::sync::atomic::AtomicBool::new(false);
            crate::usage::walk(path, meta.dev(), 1, &|_, entry| {
                if inodes.contains(&entry.ino()) {
                    found.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            })?;
            if found.into_inner() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn remove(path: &Path) -> Result<()> {
    let result = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
    };
    match result {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            fail!("deleting {}: {error}", path.display())
        }
        _ => Ok(()),
    }
}

/// `builds units prune|git-checkouts|after-build ...`: the steps setup
/// and prune run, on the folders named; the shell tests drive them this way.
/// `after-build` is what `builds cargo` starts as it exits.
pub fn cmd_units(tool: &crate::tool::Tool, args: &[String]) -> Result<()> {
    let usage = "usage: builds units prune <days> <incremental hours, 0 for all> <dry 0|1> <folder>... | \
                 git-checkouts <cargo home> <dry 0|1> <Cargo.lock>... | \
                 after-build <checkout>";
    let number = |text: &String| -> Result<u64> {
        match text.parse() {
            Ok(n) => Ok(n),
            Err(_) => fail!("{usage}"),
        }
    };
    match args.first().map(String::as_str) {
        Some("prune") if args.len() >= 4 => {
            let folders: Vec<PathBuf> = args[4..].iter().map(PathBuf::from).collect();
            let rules = prune::Ages::new(number(&args[1])?, number(&args[2])?);
            for (folder, freed_kib) in prune::prune(&rules, args[3] == "1", &folders)? {
                println!("{freed_kib}\t{}", folder.display());
            }
            Ok(())
        }
        Some("git-checkouts") if args.len() >= 3 => {
            let lockfiles: Vec<PathBuf> = args[3..].iter().map(PathBuf::from).collect();
            let freed = git_checkouts::prune_git_checkouts(
                Path::new(&args[1]),
                args[2] == "1",
                &lockfiles,
            )?;
            println!("{freed}");
            Ok(())
        }
        Some("after-build") if args.len() == 2 => {
            crate::prune::prune_after_build(tool, &crate::checkout::Checkout::find(&args[1])?)
        }
        _ => fail!("{usage}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            hashed_file("libbae_core-8fffeaeaf85dc425.rlib"),
            Some(("bae_core", "8fffeaeaf85dc425"))
        );
        assert_eq!(
            hashed_file("bae_core-8fffeaeaf85dc425"),
            Some(("bae_core", "8fffeaeaf85dc425"))
        );
        assert_eq!(
            hashed_file("bae_core-8fffeaeaf85dc425.bae_core.1c1aebe005fd4a7f-cgu.00.rcgu.o"),
            Some(("bae_core", "8fffeaeaf85dc425"))
        );
        assert_eq!(
            hashed_file("lib-8fffeaeaf85dc425.rlib"),
            Some(("lib", "8fffeaeaf85dc425"))
        );
        assert_eq!(hashed_file("libbae_bridge.a"), None);
        assert_eq!(hashed_file("bae_core-8fffeaeaf85dc425."), None);
        assert_eq!(hashed_file("bae-core-8fffeaeaf85dc425"), None);
        assert_eq!(hashed_file("app.app.abc-cgu.0.rcgu.o"), None);
        assert_eq!(
            unit_dir("bae-core-8fffeaeaf85dc425"),
            Some(("bae-core", "8fffeaeaf85dc425"))
        );
        assert_eq!(unit_dir("-8fffeaeaf85dc425"), None);
        assert_eq!(unit_dir("x-8fffeaeaf85dc42Z"), None);
    }
}
