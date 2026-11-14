//! A new side worktree's host `target`, filled from a warm checkout's.
//!
//! A side worktree whose `target` is empty spends its first clippy or test
//! build on the third-party build scripts and proc-macros sccache never
//! caches (aws-lc-sys, libsqlite3-sys with bindgen, async-trait: 57-105 s of
//! hook clippy against 12.9 s warm). The build volume is APFS, so
//! `clonefile(2)` copies another checkout's `target` in one call, sharing
//! blocks until either side writes. `builds new` does that for the host
//! `target` of the worktree it hands out, never for an app lane's folders.
//!
//! Source choice: among the repo's other checkouts (main's `target` is only
//! read) whose `target` holds a built debug profile and fits the new
//! worktree's cap, the one whose committed Cargo.lock is origin/main's (same
//! dependencies the new worktree will build), then the one with the most units
//! built (fingerprints in `debug/.fingerprint`), then the one whose last
//! `builds cargo` build finished most recently (`.after-build.log`, written as
//! each build exits). A copy is taken while holding the source's guard
//! (`units::GUARD`, what `builds cargo` holds while cargo runs) and every
//! `.cargo-lock` of its folder, exclusively and without waiting, so the clone
//! is a state cargo itself left and no build starts in the source meanwhile.
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
//! The clone is made beside its destination and renamed over the empty folder,
//! so the folder is empty or whole. Incremental folders and the rest of what
//! a side worktree's builds never read are then deleted by the same prune
//! `setup` runs.

use std::ffi::CString;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::checkout::{Checkout, Root};
use crate::evict::{cargo_locks, lock_exclusive_creating};
use crate::fail;
use crate::fail::{Context, Result};
use crate::manifest::build_dirs;
use crate::prune::{Run, prune_root};
use crate::text::note;
use crate::tool::Tool;
use crate::units::{GUARD, Guard, lock_guard_creating};

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
    let Some(locks) = cargo_locks(&target_of(&candidate.root))? else {
        return Ok(None);
    };
    Ok(Some(Held {
        candidate: candidate.clone(),
        _guard: guard,
        _locks: locks,
    }))
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
const CLONE_NOFOLLOW: u32 = 0x0001;

/// Clones <source> to <staging> (which must not exist).
fn clone_dir(source: &Path, staging: &Path) -> Result<()> {
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

fn is_empty_dir(dir: &Path) -> Result<bool> {
    Ok(std::fs::read_dir(dir)
        .context(|| format!("listing {}", dir.display()))?
        .next()
        .is_none())
}

/// The blob git has for Cargo.lock at <rev> in <dir>, or "" when <rev> has
/// none. `rev-parse --verify -q` exits 1, silently, for a name that resolves
/// to nothing; any other way for git to fail (it did not start, another exit
/// status, a signal, a broken repo) is an error, never read as "no lock".
fn lock_blob(dir: &Path, rev: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "-q", &format!("{rev}:Cargo.lock")])
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running git in {}", dir.display()))?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        Some(1) if output.stderr.is_empty() => Ok(String::new()),
        _ => fail!(
            "git failed to find Cargo.lock at {rev} in {}: {} {}",
            dir.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
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
        if !target_of(&root.path).join("debug/.fingerprint").is_dir() {
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
        let units = std::fs::read_dir(target_of(&root.path).join("debug/.fingerprint"))
            .context(|| format!("listing the fingerprints of {}", root.path.display()))?
            .count();
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
fn match_times(source_top: &Path, dest_top: &Path) -> Result<()> {
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
    let staging = root.join(format!(".target.warm.{}", crate::text::staged_suffix()));
    let _ = std::fs::remove_dir_all(&staging);
    let cloned = clone_dir(&source, &staging).and_then(|()| {
        std::fs::rename(&staging, &dest)
            .context(|| format!("moving the clone into {}", dest.display()))
    });
    if let Err(error) = cloned {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    // The source's files are read while its guard is held: no build there
    // writes them, so their mtimes are what its fingerprints were made from.
    match_times(&held.candidate.top, &checkout.top)?;
    drop(held);
    note(format!(
        "cloned {} into {}",
        source.display(),
        dest.display()
    ));
    // The source's incremental folders and the like: a side worktree's
    // builds read none of them.
    prune_root(tool, &root, checkout, Run::Real, 0, 0)
}

#[cfg(test)]
#[path = "warm_tests.rs"]
mod tests;
