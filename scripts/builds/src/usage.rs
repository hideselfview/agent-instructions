//! The disk space a folder takes, counting each block once, and when anything
//! inside it was last written.
//!
//! `du` counts a block once per file that holds it unless the files are hard
//! links of one inode. APFS clones are separate inodes sharing blocks: cargo
//! uplifts deps/libbae_bridge.a to debug/libbae_bridge.a as a clone, and `du`
//! read bae's target-macos as 9.3 GB where it took 7.3 GB. A file of at least
//! `CLONE_MIN` is measured by its physical extents (fcntl F_LOG2PHYS_EXT),
//! each counted once; a clone of a smaller file would shift a total by too
//! little to look for. Hard-linked inodes (rustc links deps/ files into
//! incremental/) count once.

use std::collections::HashSet;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use crate::fail;
use crate::fail::Result;

pub const BLOCK: u64 = 4096;
pub const CLONE_MIN: u64 = 1 << 20;
const WORKERS: usize = 8;

/// What a folder takes and its newest write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Measure {
    pub kib: u64,
    /// The newest modification time of the folder and anything inside it,
    /// seconds since 1970; None when the path is missing.
    pub newest: Option<i64>,
}

impl Measure {
    /// Did anything inside get written after <since> (seconds)?
    pub fn written_since(&self, since: i64) -> bool {
        self.newest.is_some_and(|newest| newest > since)
    }
}

/// F_LOG2PHYS_EXT's struct log2phys from <sys/fcntl.h>, packed.
#[repr(C, packed(4))]
struct Log2Phys {
    flags: u32,
    contigbytes: i64,
    devoffset: i64,
}
const F_LOG2PHYS_EXT: libc::c_int = 65;

/// (device offset, bytes) of each physical extent of the file at <path>;
/// None when the file has no plain extent map (compressed, holed) or can't
/// be opened.
pub fn extents(path: &Path, size: u64) -> Option<Vec<(i64, u64)>> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::File::open(path).ok()?;
    let mut found = Vec::new();
    let mut offset: u64 = 0;
    while offset < size {
        let mut query = Log2Phys { flags: 0, contigbytes: (size - offset) as i64, devoffset: offset as i64 };
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), F_LOG2PHYS_EXT, &mut query as *mut Log2Phys) };
        if rc == -1 {
            return None;
        }
        let contiguous = query.contigbytes;
        if contiguous <= 0 {
            break;
        }
        found.push((query.devoffset, contiguous as u64));
        offset += contiguous as u64;
    }
    if found.is_empty() { None } else { Some(found) }
}

/// The sets one measurement counts each inode and extent once across.
#[derive(Default)]
pub struct Seen {
    inodes: Mutex<HashSet<(u64, u64)>>,
    extents: Mutex<HashSet<(u64, i64)>>,
}

impl Seen {
    /// Bytes the entry at <path> adds, given what was counted before.
    pub fn bytes(&self, path: &Path, meta: &Metadata) -> u64 {
        if meta.nlink() > 1 && !meta.is_dir() {
            let key = (meta.dev(), meta.ino());
            if !self.inodes.lock().expect("no panic holding it").insert(key) {
                return 0;
            }
        }
        let allocated = meta.blocks() * 512;
        if meta.is_file() && allocated >= CLONE_MIN
            && let Some(found) = extents(path, meta.size()) {
                let mut seen = self.extents.lock().expect("no panic holding it");
                return found
                    .into_iter()
                    .filter(|(device, _)| seen.insert((meta.dev(), *device)))
                    .map(|(_, bytes)| bytes.div_ceil(BLOCK) * BLOCK)
                    .sum();
            }
        allocated
    }
}

/// Measures <path>, a file or a whole folder; a missing path takes 0. The
/// walk stays on <path>'s device and never follows a link.
#[cfg(test)]
pub fn measure(path: &Path) -> Result<Measure> {
    measure_with(path, WORKERS)
}

/// Measures each of <paths> on its own (a block two of them share counts in
/// both): one path walked several folders at a time, several paths each
/// walked by one thread.
pub fn measure_many(paths: &[PathBuf]) -> Result<Vec<Measure>> {
    if let [path] = paths {
        return Ok(vec![measure_with(path, WORKERS)?]);
    }
    crate::checkout::parallel_map(paths, |path| measure_with(path, 1)).into_iter().collect()
}

fn mtime(meta: &Metadata) -> i64 {
    meta.mtime()
}

fn measure_with(path: &Path, workers: usize) -> Result<Measure> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Measure::default()),
        Err(error) => fail!("reading {}: {error}", path.display()),
    };
    let seen = Seen::default();
    let bytes = AtomicU64::new(seen.bytes(path, &meta));
    let newest = AtomicI64::new(mtime(&meta));
    if meta.is_dir() {
        walk(path, meta.dev(), workers, &|entry_path, entry| {
            bytes.fetch_add(seen.bytes(entry_path, entry), Ordering::Relaxed);
            newest.fetch_max(mtime(entry), Ordering::Relaxed);
        })?;
    }
    Ok(Measure { kib: bytes.into_inner() / 1024, newest: Some(newest.into_inner()) })
}

/// Calls <visit> with every entry under the folder <root> (not <root>
/// itself), <workers> folders at a time, descending into folders on <dev>.
/// An entry that vanishes while it is listed is skipped: it takes nothing
/// now.
pub fn walk(root: &Path, dev: u64, workers: usize, visit: &(dyn Fn(&Path, &Metadata) + Sync)) -> Result<()> {
    struct Queue {
        folders: Vec<PathBuf>,
        busy: usize,
        error: Option<String>,
    }
    let queue = Mutex::new(Queue { folders: vec![root.to_path_buf()], busy: 0, error: None });
    let ready = Condvar::new();
    let work = || {
        loop {
            let folder = {
                let mut q = queue.lock().expect("no panic holding it");
                loop {
                    if q.error.is_some() {
                        return;
                    }
                    if let Some(folder) = q.folders.pop() {
                        q.busy += 1;
                        break folder;
                    }
                    if q.busy == 0 {
                        ready.notify_all();
                        return;
                    }
                    q = ready.wait(q).expect("no panic holding it");
                }
            };
            let mut found = Vec::new();
            let listed = list(&folder, dev, visit, &mut found);
            let mut q = queue.lock().expect("no panic holding it");
            q.busy -= 1;
            if let Err(error) = listed {
                q.error.get_or_insert(error);
            }
            q.folders.extend(found);
            ready.notify_all();
        }
    };
    std::thread::scope(|scope| {
        for _ in 1..workers.max(1) {
            scope.spawn(work);
        }
        work();
    });
    match queue.into_inner().expect("no panic holding it").error {
        Some(error) => fail!("{error}"),
        None => Ok(()),
    }
}

fn list(
    folder: &Path,
    dev: u64,
    visit: &(dyn Fn(&Path, &Metadata) + Sync),
    found: &mut Vec<PathBuf>,
) -> std::result::Result<(), String> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if gone(&error) => return Ok(()),
        Err(error) => return Err(format!("listing {}: {error}", folder.display())),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if gone(&error) => continue,
            Err(error) => return Err(format!("listing {}: {error}", folder.display())),
        };
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if gone(&error) => continue,
            Err(error) => return Err(format!("reading {}: {error}", path.display())),
        };
        visit(&path, &meta);
        if meta.is_dir() && meta.dev() == dev {
            found.push(path);
        }
    }
    Ok(())
}

/// The error a path deleted while it was being read gives.
fn gone(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
}

/// Bytes <path> takes, a file or a whole folder, counting what <seen>
/// counted before once; 0 when it is missing.
pub fn bytes_with(seen: &Seen, path: &Path) -> Result<u64> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if gone(&error) => return Ok(0),
        Err(error) => fail!("reading {}: {error}", path.display()),
    };
    let total = AtomicU64::new(seen.bytes(path, &meta));
    if meta.is_dir() {
        walk(path, meta.dev(), 1, &|child, entry| {
            total.fetch_add(seen.bytes(child, entry), Ordering::Relaxed);
        })?;
    }
    Ok(total.into_inner())
}

/// Bytes deleting <path> frees: the blocks of its files nothing else hard
/// links (rustc hard-links a unit's deps files into its incremental folder,
/// and those stay with the unit).
pub fn freeable(path: &Path) -> Result<u64> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if gone(&error) => return Ok(0),
        Err(error) => fail!("reading {}: {error}", path.display()),
    };
    if !meta.is_dir() {
        return Ok(if meta.nlink() == 1 { meta.blocks() * 512 } else { 0 });
    }
    let total = AtomicU64::new(0);
    walk(path, meta.dev(), 1, &|_, entry| {
        if !entry.is_dir() && entry.nlink() == 1 {
            total.fetch_add(entry.blocks() * 512, Ordering::Relaxed);
        }
    })?;
    Ok(total.into_inner())
}

/// `builds du <path>...`: KiB per path, one line each, in order.
pub fn cmd_du(args: &[String]) -> Result<()> {
    if args.is_empty() {
        fail!("usage: builds du <path>...");
    }
    let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    for measured in measure_many(&paths)? {
        println!("{}", measured.kib);
    }
    Ok(())
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
