//! A new side worktree's host `target`, filled from a warm checkout's.
//!
//! A side worktree whose `target` is empty spends its first clippy or test
//! build on the third-party build scripts and proc-macros sccache never
//! caches (aws-lc-sys, libsqlite3-sys with bindgen, async-trait: 57-105 s of
//! hook clippy against 12.9 s warm). The build volume is APFS, so
//! `clonefile(2)` copies another checkout's `target` in one call, sharing
//! blocks until either side writes. `builds new` does that for the host
//! `target` of the worktree it hands out. A new app checkout of a lane clones
//! its lane's cargo folders from the lane's warmest checkout by the same
//! means (`clone_lane`).
//!
//! Source choice: among the repo's other checkouts (main's `target` is only
//! read) whose `target` holds a built debug profile and fits the new
//! worktree's cap, the one whose committed Cargo.lock is origin/main's (same
//! dependencies the new worktree will build), then the one with the most units
//! built (units with a fingerprint folder in `debug`, as prune lists them),
//! then the one whose last `builds cargo` build finished most recently
//! (`.after-build.log`, written as each build exits). A copy is taken while
//! holding the source's guard (`units::GUARD`, what `builds cargo` holds
//! while cargo runs) and every `.cargo-lock` of its folder, exclusively and
//! without waiting, so the clone is a state cargo itself left and no build
//! starts in the source meanwhile.
//! A source a build holds is passed over for the next; when every candidate is
//! building, the call waits on the best one's guard, which `builds cargo`
//! drops when its cargo exits (no time limit, no retry).
//!
//! Cargo calls a source file changed when its mtime is newer than the build's
//! record, and a fresh checkout stamps every file with the checkout's time, so
//! the cloned fingerprints would all read stale: every workspace crate's build
//! script reruns, and each crate above it is compiled again (path crates are
//! cached by sccache per path, so a new worktree misses). After the clone,
//! every tracked file of the new worktree that is byte for byte the source
//! checkout's gets the source's mtime, which makes the new tree look as the
//! source's did when it was built; a file that differs keeps its new mtime and
//! is rebuilt, as it must be.
//!
//! A build script that reads a path from its environment (ffmpeg-sys-next's
//! FFMPEG_DIR) prints it in a `cargo:` line, and cargo's record of the run
//! does not name the variable, so the clone would keep the source's path in
//! its links and fail once that checkout is removed. Before the clone is
//! placed, the record of each run whose `cargo:` lines name the source's
//! working tree or build root is deleted from it (`forget_runs_naming`), and
//! cargo runs those scripts again.
//!
//! The clone is made beside its destination and renamed over the empty folder,
//! so the folder is empty or whole. Every incremental folder is then deleted,
//! whatever its age: the clone's folders were written by the source's builds
//! (a new directory in a clone has a new mtime, so each would otherwise pass
//! for written just now and stay for the idle hours), and a side worktree's
//! builds name other crate paths. The same prune `setup` runs then deletes the
//! rest of what a side worktree's builds never read.

use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::checkout::{Checkout, Root, resolve_revision};
use crate::evict::{cargo_locks, lock_exclusive_creating};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::manifest::{BuildDir, build_dirs};
use crate::prune::{Run, prune_root};
use crate::text::note;
use crate::tool::Tool;
use crate::units::prune::Ages;
use crate::units::unit::built_count;
use crate::units::{self, GUARD, Guard, is_profile_dir, lock_guard_creating};

/// A checkout whose host `target` could be cloned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Its build root.
    pub root: PathBuf,
    /// Its working tree: the sources its `target` was built from.
    pub top: PathBuf,
    /// Does its committed Cargo.lock equal origin/main's?
    pub same_lock: bool,
    /// How many units its debug profile holds a fingerprint for: a target a
    /// recent small build touched holds a few hundred, one that has run the
    /// hook's clippy holds the whole dependency tree.
    pub units: usize,
    /// When its last `builds cargo` build finished, in seconds since 1970; 0
    /// when none is recorded.
    pub finished: i64,
}

/// Best source first: origin/main's dependencies, then the most units built
/// (the clone saves what was built, so a recent build of one small crate must
/// not outrank a target that holds the whole tree), then the newest build.
pub fn rank(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        (
            !a.same_lock,
            std::cmp::Reverse(a.units),
            -a.finished,
            &a.root,
        )
            .cmp(&(
                !b.same_lock,
                std::cmp::Reverse(b.units),
                -b.finished,
                &b.root,
            ))
    });
}

/// What a taken source holds until the clone is made.
pub struct Held {
    pub candidate: Candidate,
    _hold: Hold,
}

/// A source checkout's guard and the cargo locks of some of its folders.
pub struct Hold {
    _guard: Guard,
    _locks: Vec<File>,
}

fn target_of(root: &Path) -> PathBuf {
    root.join("target")
}

/// The source's guard and cargo locks taken without waiting; None when a
/// build holds either.
fn try_hold(candidate: &Candidate) -> Result<Option<Held>> {
    let guard = match lock_guard_creating(&candidate.root)? {
        Guard::Busy => return Ok(None),
        guard => guard,
    };
    hold_rest(candidate, guard)
}

fn hold_rest(candidate: &Candidate, guard: Guard) -> Result<Option<Held>> {
    Ok(
        hold_folders(guard, &[target_of(&candidate.root)])?.map(|hold| Held {
            candidate: candidate.clone(),
            _hold: hold,
        }),
    )
}

/// <guard> and the cargo locks of each of <folders>, each taken without
/// waiting; None when a build holds any.
fn hold_folders(guard: Guard, folders: &[PathBuf]) -> Result<Option<Hold>> {
    let mut locks = Vec::new();
    for folder in folders {
        let Some(held) = cargo_locks(folder)? else {
            return Ok(None);
        };
        locks.extend(held);
    }
    Ok(Some(Hold {
        _guard: guard,
        _locks: locks,
    }))
}

/// The guard of the checkout whose build root is <root> and the cargo locks of
/// each of <folders>, taken without waiting; None when a build holds any.
pub(crate) fn try_hold_folders(root: &Path, folders: &[PathBuf]) -> Result<Option<Hold>> {
    match lock_guard_creating(root)? {
        Guard::Busy => Ok(None),
        guard => hold_folders(guard, folders),
    }
}

/// Clones <from> to <to>, an empty folder, as one step: the clone is made
/// beside <to>, its build script runs that name <source_paths> are forgotten
/// (`forget_runs_naming`), and it is renamed over <to>, so <to> is empty or
/// whole. A clone that fails is deleted, and a failure to delete it is part of
/// the error.
fn clone_into_place(from: &Path, to: &Path, source_paths: &[&Path]) -> Result<()> {
    let staging = to.with_file_name(format!(
        ".{}.warm.{}",
        crate::checkout::file_name(to),
        crate::text::staged_suffix()
    ));
    let cloned = clone_dir(from, &staging)
        .and_then(|()| units::source_paths::forget_runs_naming(&staging, source_paths))
        .and_then(|()| {
            std::fs::rename(&staging, to)
                .context(|| format!("moving the clone into {}", to.display()))
        });
    if cloned.is_err() {
        let removed = match std::fs::remove_dir_all(&staging) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).context(|| format!("deleting the failed clone {}", staging.display()))
            }
            _ => Ok(()),
        };
        return crate::fail::with_cleanup(cloned, removed);
    }
    Ok(())
}

/// Takes the best candidate no build holds; when every one is held, waits for
/// the best one's guard. None when there is no candidate, or when cargo typed
/// by hand (it takes no guard) holds the best one's folder after its guard
/// came free.
pub fn acquire(ranked: &[Candidate]) -> Result<Option<Held>> {
    for candidate in ranked {
        if let Some(held) = try_hold(candidate)? {
            return Ok(Some(held));
        }
    }
    let Some(best) = ranked.first() else {
        return Ok(None);
    };
    let file = lock_exclusive_creating(&best.root.join(GUARD))?;
    hold_rest(best, Guard::Held(file))
}

/// clonefile(2)'s CLONE_NOFOLLOW (sys/clonefile.h): a link at the top is cloned
/// as a link, never followed.
#[cfg(target_os = "macos")]
const CLONE_NOFOLLOW: u32 = 0x0001;

/// Off macOS no side worktree is handed out: `builds new` runs on the Mac
/// the person works on.
#[cfg(not(target_os = "macos"))]
pub(crate) fn clone_dir(source: &Path, _staging: &Path) -> Result<()> {
    fail!(
        "cloning {} needs macOS's clonefile, and this machine runs Linux",
        source.display()
    )
}

/// Clones <source> to <staging> (which must not exist).
#[cfg(target_os = "macos")]
pub(crate) fn clone_dir(source: &Path, staging: &Path) -> Result<()> {
    use std::ffi::CString;
    let from = CString::new(source.as_os_str().as_bytes()).expect("a path holds no NUL");
    let to = CString::new(staging.as_os_str().as_bytes()).expect("a path holds no NUL");
    if unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), CLONE_NOFOLLOW) } != 0 {
        fail!(
            "cloning {} to {}: {}",
            source.display(),
            staging.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

pub(crate) fn is_empty_dir(dir: &Path) -> Result<bool> {
    Ok(std::fs::read_dir(dir)
        .context(|| format!("listing {}", dir.display()))?
        .next()
        .is_none())
}

/// The blob Git has for Cargo.lock at <rev> in <dir>, or None when it has none.
fn lock_blob(dir: &Path, rev: &str) -> Result<Option<String>> {
    resolve_revision(dir, &format!("{rev}:Cargo.lock"))
}

/// Every other checkout of <checkout>'s repo whose `target` could be cloned
/// into a folder capped at <cap_kib>, unranked.
fn candidates(tool: &Tool, checkout: &Checkout, cap_kib: u64) -> Result<Vec<Candidate>> {
    let dest_root = checkout.build_root(&tool.places);
    let wanted = lock_blob(&checkout.top, "origin/main")?;
    let mut found: Vec<(Root, Checkout)> = Vec::new();
    for root in tool.roots()? {
        let Some(live) = root.live.clone() else {
            continue;
        };
        if live.main != checkout.main || root.path == dest_root {
            continue;
        }
        if !is_profile_dir(&target_of(&root.path).join("debug"))? {
            continue;
        }
        found.push((root, live));
    }
    let folders: Vec<PathBuf> = found
        .iter()
        .map(|(root, _)| target_of(&root.path))
        .collect();
    let sizes = tool.measure_all(&folders)?;
    let mut candidates = Vec::new();
    for ((root, live), size) in found.into_iter().zip(sizes) {
        // A clone over the cap would be deleted whole by the next prune.
        if size.kib > cap_kib {
            continue;
        }
        let finished = std::fs::metadata(root.path.join(".after-build.log"))
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs() as i64);
        let units = built_count(&target_of(&root.path).join("debug"))?;
        candidates.push(Candidate {
            top: live.top.clone(),
            units,
            same_lock: lock_blob(&live.top, "HEAD")? == wanted,
            root: root.path,
            finished,
        });
    }
    Ok(candidates)
}

/// Gives each tracked file of <dest_top> that is byte for byte the same file of
/// <source_top> that file's mtime. A file the source lacks, a link, or a file
/// whose content differs keeps its own.
pub(crate) fn match_times(source_top: &Path, dest_top: &Path) -> Result<()> {
    let listed = Command::new("git")
        .arg("-C")
        .arg(dest_top)
        .args(["ls-files", "-z"])
        .stdin(Stdio::null())
        .output()
        .context(|| format!("listing the files of {}", dest_top.display()))?;
    if !listed.status.success() {
        fail!(
            "git ls-files in {}: {} {}",
            dest_top.display(),
            listed.status,
            String::from_utf8_lossy(&listed.stderr).trim()
        );
    }
    for name in listed
        .stdout
        .split(|byte| *byte == 0)
        .filter(|n| !n.is_empty())
    {
        let name = Path::new(std::ffi::OsStr::from_bytes(name));
        let (from, to) = (source_top.join(name), dest_top.join(name));
        let (Ok(from_meta), Ok(to_meta)) = (
            std::fs::symlink_metadata(&from),
            std::fs::symlink_metadata(&to),
        ) else {
            continue;
        };
        if !from_meta.is_file() || !to_meta.is_file() || from_meta.len() != to_meta.len() {
            continue;
        }
        let (Ok(from_bytes), Ok(to_bytes)) = (std::fs::read(&from), std::fs::read(&to)) else {
            continue;
        };
        if from_bytes != to_bytes {
            continue;
        }
        let when = from_meta
            .modified()
            .context(|| format!("reading the mtime of {}", from.display()))?;
        File::open(&to)
            .and_then(|file| file.set_modified(when))
            .context(|| format!("setting the mtime of {}", to.display()))?;
    }
    Ok(())
}

/// Fills <checkout>'s host `target` from a warm checkout's when it is empty
/// (a new worktree, or a free one whose folder prune, a cap or the floor
/// deleted). Nothing to clone from is not a failure: the first build is cold,
/// as it always was.
pub fn fill(tool: &Tool, checkout: &Checkout) -> Result<()> {
    let Some(entry) = build_dirs(&checkout.top)?
        .into_iter()
        .find(|entry| entry.path == "target" && entry.lane.is_none())
    else {
        return Ok(());
    };
    let root = checkout.build_root(&tool.places);
    let dest = target_of(&root);
    if !dest.is_dir() || !is_empty_dir(&dest)? {
        return Ok(());
    }
    let mut ranked = candidates(tool, checkout, entry.cap_kib(false))?;
    rank(&mut ranked);
    let Some(held) = acquire(&ranked)? else {
        note(format!(
            "no warm host build of {} to clone; its first build is cold",
            checkout.repo()
        ));
        return Ok(());
    };
    let source = target_of(&held.candidate.root);
    clone_into_place(&source, &dest, &[&held.candidate.top, &held.candidate.root])?;
    // The source's files are read while its guard is held: no build there
    // writes them, so their mtimes are what its fingerprints were made from.
    match_times(&held.candidate.top, &checkout.top)?;
    units::source_paths::forget_runs_naming(&dest, &[&held.candidate.top, &held.candidate.root])?;
    drop(held);
    note(format!(
        "cloned {} into {}",
        source.display(),
        dest.display()
    ));
    // The source's incremental folders and the like: a side worktree's
    // builds read none of them. 0 incremental hours deletes every one.
    units::prune::prune(
        &Ages::new(tool.rules.unused_days, 0),
        Run::Real,
        std::slice::from_ref(&dest),
    )?;
    prune_root(
        tool,
        &root,
        checkout,
        Run::Real,
        0,
        0,
        crate::prune::Pruner::Setup,
    )
}

/// `builds app-clone mac|android <source> <dest>`: `clone_lane` for two
/// checkouts of this machine, which a host runs for the new app checkout that
/// this Mac's build makes there (app_remote.rs).
pub fn cmd_app_clone(tool: &Tool, args: &[String]) -> Result<()> {
    let [lane, source, dest] = args else {
        fail!("usage: builds app-clone mac|android <source checkout> <new checkout>")
    };
    let lane = Lane::parse_or_fail(lane)?;
    tool.places.require_volume()?;
    clone_lane(tool, lane, &Checkout::find(source)?, &Checkout::find(dest)?)
}

/// The folders of <dest_root> that a new checkout of <lane> starts from a clone
/// of <source_root>'s: the build folders the lane's builds write that hold a
/// cargo build in the source (a built debug profile) and are empty in the
/// destination. A cargo build survives a move to another checkout path (see
/// `clone_lane`); Xcode's trees and Gradle's build folders do not, so no other
/// folder is cloned.
pub fn cloned_folders(
    dirs: &[BuildDir],
    lane: Lane,
    source_root: &Path,
    dest_root: &Path,
) -> Result<Vec<BuildDir>> {
    let mut folders = Vec::new();
    for dir in dirs {
        if !dir.is_app_in(Some(lane)) {
            continue;
        }
        let dest = dest_root.join(&dir.path);
        if !is_profile_dir(&source_root.join(&dir.path).join("debug"))?
            || !dest.is_dir()
            || !is_empty_dir(&dest)?
        {
            continue;
        }
        folders.push(dir.clone());
    }
    Ok(folders)
}

/// Fills the empty cargo build folders of <dest>, a new app checkout of
/// <lane>, with copy-on-write clones of <source>'s, the lane's warmest
/// checkout on this host. The caller holds <source>'s lane lock, so no app
/// build runs there; this function also holds its guard and cargo locks, as
/// `fill` does, and ends without cloning when a build holds them.
///
/// Only cargo's folders are cloned. Cargo names a unit by a hash that leaves
/// out where the workspace is, and the mtimes that `match_times` copies make
/// its fingerprints read as fresh, which is what lets `fill` clone a host
/// `target`. Xcode's trees are named by their own real path and hold the
/// checkout's path in the build description (XCBuildData's manifest) and in
/// every compile command, so a moved tree recompiles everything; SwiftPM's
/// SourcePackages holds the checkout's path in workspace-state.json and in
/// each checkout's git alternates, so a clone would point into the source and
/// break when builds removes it. Gradle keeps each task's recorded outputs by
/// absolute path, so a moved build folder reruns its tasks.
pub fn clone_lane(tool: &Tool, lane: Lane, source: &Checkout, dest: &Checkout) -> Result<()> {
    let source_root = source.build_root(&tool.places);
    let dest_root = dest.build_root(&tool.places);
    let dirs = build_dirs(&dest.top)?;
    let candidates = cloned_folders(&dirs, lane, &source_root, &dest_root)?;
    if candidates.is_empty() {
        note(format!(
            "{} has no cargo build to clone into {}; its first build is cold",
            source.top.display(),
            dest.top.display()
        ));
        return Ok(());
    }
    let paths: Vec<PathBuf> = candidates
        .iter()
        .map(|dir| source_root.join(&dir.path))
        .collect();
    let sizes = tool.measure_all(&paths)?;
    // A clone over the cap would be deleted whole by the next prune.
    let mut folders = Vec::new();
    for (dir, size) in candidates.into_iter().zip(&sizes) {
        if size.kib <= dir.cap_kib(true) {
            folders.push(dir);
        } else {
            note(format!(
                "{} is {} in {}, over the {} that {} may hold, so it is not cloned",
                dir.path,
                crate::text::human(size.kib),
                source.top.display(),
                crate::text::human(dir.cap_kib(true)),
                dest.top.display()
            ));
        }
    }
    if folders.is_empty() {
        return Ok(());
    }
    let paths: Vec<PathBuf> = folders
        .iter()
        .map(|dir| source_root.join(&dir.path))
        .collect();
    let Some(held) = try_hold_folders(&source_root, &paths)? else {
        note(format!(
            "cargo runs in {}, so nothing is cloned from it; {}'s first build is cold",
            source.top.display(),
            dest.top.display()
        ));
        return Ok(());
    };
    let mut cloned = Vec::new();
    for dir in &folders {
        let (from, to) = (source_root.join(&dir.path), dest_root.join(&dir.path));
        clone_into_place(&from, &to, &[&source.top, &source_root])?;
        note(format!("cloned {} into {}", from.display(), to.display()));
        cloned.push(to);
    }
    // The source's files are read while its guard is held: no build there
    // writes them, so their mtimes are what its fingerprints were made from.
    match_times(&source.top, &dest.top)?;
    drop(held);
    // An app checkout's builds run without incremental compilation.
    units::prune::prune(&Ages::new(tool.rules.unused_days, 0), Run::Real, &cloned)?;
    Ok(())
}

#[cfg(test)]
#[path = "warm_tests.rs"]
mod tests;
