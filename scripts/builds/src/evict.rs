//! Deleting a folder whole, unless a build is using it.
//!
//! A build in another process can start at any moment, so "in use" is
//! decided while holding what a build holds. Cargo keeps an exclusive flock on
//! <target>/<profile>/.cargo-lock (and <target>/<triple>/<profile>/.cargo-lock)
//! while it compiles: those locks are taken without waiting, and a cargo
//! already holding one makes the folder in use. While they are held, no cargo
//! can start writing; a cargo that queues on one gets it after the rename and
//! writes through the link into the new, empty folder. Folders without cargo
//! locks (Xcode's DerivedData, Gradle, dotnet, lake) count as in use while any
//! process holds a file or working directory inside them; that is checked
//! last, immediately before the rename. A folder inside a checkout's build
//! root is in use too while a `builds cargo` build holds that checkout's
//! guard (`units::GUARD`): cargo lets go of its lock before `cargo test`
//! runs the tests it built. The guard is taken, creating its file when a
//! checkout has had no `builds cargo` build yet, so a build starting after the
//! check waits for the guard until the folder has been moved, never uses a
//! folder judged idle.

use std::ffi::CString;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result, with_move_cleanup};
use crate::places::Places;
use crate::process::Detached;

/// How the folder's space comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Removed by a background `rm -rf` after the rename.
    Background,
    /// Removed before `evict` returns, for a caller that reads the free space
    /// next.
    Now,
}

/// Whether an empty folder goes back where the deleted one was, so a link to
/// it stays valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recreate {
    Yes,
    No,
}

/// What became of a folder builds set out to delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evicted {
    /// builds moved it away.
    Deleted,
    /// A build or a process uses it, so it stays.
    InUse,
    /// Something else deleted it before builds could move it.
    Gone,
}

/// Deletes <dir> whole unless a build is using it: renames it into the trash
/// (instant; see `trash`) and removes it as <removal> says. The trash is
/// <root>/.trash on the build volume, and for a folder on another volume a new
/// folder beside it, since a rename can't cross volumes. Deletes nothing when the
/// folder is in use, as <system>'s lsof says, or gone. A folder in a build
/// root goes through roots.rs `delete`, which adds its root to the command's
/// changed roots.
pub fn evict(
    places: &Places,
    system: &crate::system::System,
    dir: &Path,
    recreate: Recreate,
    removal: Removal,
) -> Result<Evicted> {
    displace(places, system, dir, Change::Move(recreate, removal))
}

/// Prepares a replacement and exchanges it with the live folder in one
/// rename. After the exchange, only the old contents remain in the trash.
pub fn replace(
    places: &Places,
    system: &crate::system::System,
    dir: &Path,
    prepare: &dyn Fn(&Path, &Path) -> Result<()>,
) -> Result<Evicted> {
    displace(places, system, dir, Change::Replace(prepare))
}

enum Change<'a> {
    Move(Recreate, Removal),
    Replace(&'a dyn Fn(&Path, &Path) -> Result<()>),
}

enum EvictionSlot {
    Trash(crate::trash::Slot),
    Sibling(PathBuf),
}

impl EvictionSlot {
    fn make(places: &Places, parent: &Path) -> Result<Self> {
        if device(parent)? == device(&places.root)? {
            Ok(Self::Trash(crate::trash::Slot::make(places)?))
        } else {
            Ok(Self::Sibling(make_temp_dir(parent, ".builds-deleting.")?))
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Trash(slot) => &slot.path,
            Self::Sibling(path) => path,
        }
    }

    fn remove(self, places: &Places, removal: Removal) -> Result<()> {
        match (self, removal) {
            (Self::Trash(slot), Removal::Now) => {
                slot.remove_now();
                Ok(())
            }
            (Self::Trash(slot), Removal::Background) => slot.remove_background(),
            (Self::Sibling(path), removal) => remove(places, &path, removal).map(drop),
        }
    }

    /// Removes the slot of a replacement that did not happen. A trash slot
    /// that cannot be emptied stays for the next emptier; a folder beside the
    /// cache has no emptier, so its failure reaches the caller.
    fn discard(self) -> Result<()> {
        match self {
            Self::Trash(slot) => {
                slot.remove_now();
                Ok(())
            }
            Self::Sibling(path) => {
                std::fs::remove_dir_all(&path).context(|| format!("removing {}", path.display()))
            }
        }
    }
}

/// A folder builds may move now, with what keeps builds out of it until it
/// has.
struct Claimed {
    folder: PathBuf,
    _guard: crate::units::Guard,
    locks: Vec<File>,
    slot: EvictionSlot,
}

/// Claims <dir> for <change>: None while a build or a process uses it, as
/// <system>'s lsof says.
fn claim(
    places: &Places,
    system: &crate::system::System,
    dir: &Path,
    change: &Change<'_>,
) -> Result<Option<Claimed>> {
    let folder = std::fs::canonicalize(dir).context(|| format!("resolving {}", dir.display()))?;
    let parent = folder.parent().expect("a folder to delete has a parent");
    let guard = match crate::units::lock_guard_creating(&folder)? {
        crate::units::Guard::Busy => return Ok(None),
        guard => guard,
    };
    let Some(locks) = cargo_locks(&folder)? else {
        return Ok(None);
    };
    if matches!(change, Change::Move(..)) && held_by_a_process(system, &folder)? {
        return Ok(None);
    }
    let slot = EvictionSlot::make(places, parent)?;
    Ok(Some(Claimed {
        folder,
        _guard: guard,
        locks,
        slot,
    }))
}

fn displace(
    places: &Places,
    system: &crate::system::System,
    dir: &Path,
    change: Change<'_>,
) -> Result<Evicted> {
    let claimed = match claim(places, system, dir, &change) {
        Ok(Some(claimed)) => claimed,
        Ok(None) => return Ok(Evicted::InUse),
        // A program may delete its folder at any moment after a caller listed
        // it, so each step of the claim may fail on it. builds has moved
        // nothing yet, so a folder that is missing now is one something else
        // deleted.
        Err(_) if crate::whole_file::lstat(dir)?.is_none() => return Ok(Evicted::Gone),
        Err(error) => return Err(error),
    };
    let Claimed {
        folder,
        _guard,
        locks,
        slot,
    } = claimed;
    let name = folder.file_name().expect("a folder to delete has a name");
    match change {
        Change::Move(recreate, removal) => {
            let moved = move_in(&folder, slot.path(), name, recreate)?;
            drop(locks);
            slot.remove(places, removal)?;
            Ok(if moved {
                Evicted::Deleted
            } else {
                Evicted::Gone
            })
        }
        Change::Replace(prepare) => {
            let replacement = slot.path().join(name);
            let moved = replace_from(system, &folder, &replacement, prepare);
            drop(locks);
            let cleanup = if matches!(moved, Ok(true)) {
                slot.remove(places, Removal::Background)
            } else {
                slot.discard()
            };
            Ok(if with_move_cleanup(moved, cleanup)? {
                Evicted::Deleted
            } else {
                Evicted::InUse
            })
        }
    }
}

fn replace_from(
    system: &crate::system::System,
    folder: &Path,
    replacement: &Path,
    prepare: &dyn Fn(&Path, &Path) -> Result<()>,
) -> Result<bool> {
    std::fs::create_dir(replacement).context(|| format!("creating {}", replacement.display()))?;
    prepare(folder, replacement)?;
    if held_by_a_process(system, folder)? {
        return Ok(false);
    }
    // A writer can change a copied file and close it before the check above.
    // The live folder then holds a newer file than the replacement, so the
    // exchange would send the newer file to the trash.
    if let Some(changed) = changed_since_copy(folder, replacement)? {
        fail!(
            "{} changed while builds copied it; the folder stays as it is",
            changed.display()
        );
    }
    exchange(folder, replacement)?;
    Ok(true)
}

/// What <read> returns for <path>; None when <path> is gone, any other
/// failure to the caller.
fn read_if_present<T>(
    path: &Path,
    read: impl Fn(&Path) -> std::io::Result<T>,
) -> Result<Option<T>> {
    match read(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context(|| format!("reading {}", path.display())),
    }
}

/// The first file of <replacement> whose live counterpart under <live> holds
/// other bytes or another link target.
fn changed_since_copy(live: &Path, replacement: &Path) -> Result<Option<PathBuf>> {
    for entry in
        std::fs::read_dir(replacement).context(|| format!("listing {}", replacement.display()))?
    {
        let entry = entry.context(|| format!("listing {}", replacement.display()))?;
        let copy = entry.path();
        let original = live.join(entry.file_name());
        let kind = entry
            .file_type()
            .context(|| format!("reading {}", copy.display()))?;
        let changed = if kind.is_dir() {
            if !original.is_dir() {
                Some(original)
            } else {
                changed_since_copy(&original, &copy)?
            }
        } else if kind.is_symlink() {
            (read_if_present(&copy, |path: &Path| std::fs::read_link(path))?
                != read_if_present(&original, |path: &Path| std::fs::read_link(path))?)
            .then_some(original)
        } else {
            (read_if_present(&copy, |path: &Path| std::fs::read(path))?
                != read_if_present(&original, |path: &Path| std::fs::read(path))?)
            .then_some(original)
        };
        if changed.is_some() {
            return Ok(changed);
        }
    }
    Ok(None)
}

fn exchange(folder: &Path, replacement: &Path) -> Result<()> {
    let live = CString::new(folder.as_os_str().as_bytes()).expect("a path holds no NUL");
    let staged = CString::new(replacement.as_os_str().as_bytes()).expect("a path holds no NUL");
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(live.as_ptr(), staged.as_ptr(), libc::RENAME_SWAP) };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            live.as_ptr(),
            libc::AT_FDCWD,
            staged.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    Err(crate::fail::Fail::Move {
        from: folder.to_path_buf(),
        to: replacement.to_path_buf(),
        error: std::io::Error::last_os_error(),
    })
}

/// Renames <folder> into <slot>: false, moving nothing, when another process
/// renamed it away first. Two processes can pick the same folder (a DerivedData
/// entry has no lock of its own), and the rename decides which one deletes it.
pub(crate) fn move_in(
    folder: &Path,
    slot: &Path,
    name: &std::ffi::OsStr,
    recreate: Recreate,
) -> Result<bool> {
    let moved = match move_to_slot(folder, slot, name) {
        Err(error) => {
            let cleanup =
                std::fs::remove_dir(slot).context(|| format!("removing {}", slot.display()));
            return with_move_cleanup(Err(error), cleanup);
        }
        Ok(moved) => moved,
    };
    if !moved {
        return Ok(false);
    }
    if recreate == Recreate::Yes {
        std::fs::create_dir(folder).context(|| format!("recreating {}", folder.display()))?;
    }
    Ok(true)
}

/// Moves an entry into its slot. A concurrent removal returns false; a
/// filesystem failure reaches the caller that owns deletion policy.
pub(crate) fn move_to_slot(folder: &Path, slot: &Path, name: &std::ffi::OsStr) -> Result<bool> {
    let to = slot.join(name);
    match std::fs::rename(folder, &to) {
        Ok(()) => Ok(true),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && !folder.exists()
                && slot.is_dir() =>
        {
            Ok(false)
        }
        Err(error) => Err(crate::fail::Fail::Move {
            from: folder.to_path_buf(),
            to,
            error,
        }),
    }
}

fn device(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)
        .context(|| format!("reading {}", path.display()))?
        .dev())
}

/// Every .cargo-lock in <folder> and two levels below, each taken without
/// waiting; None as soon as one is held by a build.
pub(crate) fn cargo_locks(folder: &Path) -> Result<Option<Vec<File>>> {
    let mut held = Vec::new();
    let mut pending = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => fail!("listing {}: {error}", dir.display()),
        };
        for entry in entries {
            let entry = entry.context(|| format!("listing {}", dir.display()))?;
            let kind = entry
                .file_type()
                .context(|| format!("reading {}", entry.path().display()))?;
            if entry.file_name() == ".cargo-lock" && kind.is_file() {
                match try_lock(&entry.path())? {
                    Some(file) => held.push(file),
                    None => return Ok(None),
                }
            } else if kind.is_dir() && depth < 2 {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    Ok(Some(held))
}

/// Takes the exclusive flock on <path> without waiting: the open file, or
/// None while another process holds it.
pub fn try_lock(path: &Path) -> Result<Option<File>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .context(|| format!("opening {}", path.display()))?;
    Ok(crate::flock::try_lock(&file, libc::LOCK_EX, path)?.then_some(file))
}

/// The paths of the files, working directories and mapped files that
/// processes other than this one hold (`lsof -n -Fpn`), read once.
pub struct HeldPaths(Vec<Vec<u8>>);

impl HeldPaths {
    /// What <system>'s lsof lists.
    pub fn read(system: &crate::system::System) -> Result<HeldPaths> {
        let me = std::process::id().to_string();
        let output = system
            .command(crate::system::Program::Lsof)?
            .args(["-n", "-Fpn"])
            .stdin(Stdio::null())
            .output()
            .context(|| "running lsof".to_string())?;
        if !output.status.success() {
            fail!(
                "lsof failed ({}), so builds cannot tell which files processes hold: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let mut names = Vec::new();
        let mut pid: &[u8] = b"";
        for line in output.stdout.split(|byte| *byte == b'\n') {
            match line.first() {
                Some(b'p') => pid = &line[1..],
                Some(b'n') if pid != me.as_bytes() => names.push(line[1..].to_vec()),
                _ => {}
            }
        }
        Ok(HeldPaths(names))
    }

    /// Does a process hold a file at or inside <folder>?
    pub fn holds(&self, folder: &Path) -> bool {
        let folder = folder.as_os_str().as_bytes();
        self.0.iter().any(|name| {
            name == folder || (name.starts_with(folder) && name.get(folder.len()) == Some(&b'/'))
        })
    }
}

/// Does any process but this one hold a file, a working directory or a
/// mapped file at or inside <folder>? (<system>'s `lsof -n -Fpn`)
pub fn held_by_a_process(system: &crate::system::System, folder: &Path) -> Result<bool> {
    Ok(HeldPaths::read(system)?.holds(folder))
}

/// A new folder in <dir> named <prefix> and eight random characters.
pub fn make_temp_dir(dir: &Path, prefix: &str) -> Result<PathBuf> {
    let template = dir.join(format!("{prefix}XXXXXXXX"));
    let template = CString::new(template.as_os_str().as_bytes()).expect("a path holds no NUL");
    let mut bytes = template.into_bytes_with_nul();
    let made = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
    if made.is_null() {
        fail!(
            "creating a folder in {}: {}",
            dir.display(),
            std::io::Error::last_os_error()
        );
    }
    bytes.pop();
    Ok(PathBuf::from(std::ffi::OsString::from(
        std::ffi::OsStr::from_bytes(&bytes),
    )))
}

/// Makes a new folder in <dir> named <prefix> and eight random characters
/// (`make_temp_dir`), runs <work> with it and then removes it, whatever
/// <work> returned. Fails with <work>'s error, the removal's, or both.
pub fn in_temp_dir<T>(
    dir: &Path,
    prefix: &str,
    work: impl FnOnce(&Path) -> Result<T>,
) -> Result<T> {
    let folder = make_temp_dir(dir, prefix)?;
    let worked = work(&folder);
    let removed =
        std::fs::remove_dir_all(&folder).context(|| format!("removing {}", folder.display()));
    match (worked, removed) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(also)) => fail!("{}; {}", error.text(), also.text()),
    }
}

/// Removes <slot> now (warning, and leaving it, when it cannot be finished),
/// or detaches `rm -rf` on it, so it finishes after this process and its
/// terminal are gone.
fn remove(places: &Places, slot: &Path, removal: Removal) -> Result<bool> {
    match removal {
        Removal::Now => {
            return Ok(crate::trash::remove_or_warn(slot));
        }
        Removal::Background => {
            background_rm(places, slot)?;
        }
    }
    Ok(true)
}

/// `rm -rf <path>` detached from this process.
fn background_rm(places: &Places, path: &Path) -> Result<()> {
    let mut command = Command::new("rm");
    command
        .arg("-rf")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let purpose = format!("remove {}", path.display());
    Detached::start(&places.root, &mut command, &purpose)?;
    Ok(())
}

#[cfg(test)]
#[path = "evict_tests.rs"]
mod tests;
