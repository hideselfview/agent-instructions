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
//! dependencies the new worktree will build), then the one whose last
//! `builds cargo` build finished most recently (`.after-build.log`, written as
//! each build exits). A copy is taken while holding the source's guard
//! (`units::GUARD`, what `builds cargo` holds while cargo runs) and every
//! `.cargo-lock` of its folder, exclusively and without waiting, so the clone
//! is a state cargo itself left and no build starts in the source meanwhile.
//! A source a build holds is passed over for the next; when every candidate is
//! building, the call waits on the best one's guard, which `builds cargo`
//! drops when its cargo exits (no time limit, no retry).
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
    /// Does its committed Cargo.lock equal origin/main's?
    pub same_lock: bool,
    /// When its last `builds cargo` build finished, in seconds since 1970; 0
    /// when none is recorded.
    pub finished: i64,
}

/// Best source first: origin/main's dependencies, then the newest build.
pub fn rank(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        (!a.same_lock, -a.finished, &a.root).cmp(&(!b.same_lock, -b.finished, &b.root))
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
    let Some(locks) = cargo_locks(&target_of(&candidate.root))? else { return Ok(None) };
    Ok(Some(Held { candidate: candidate.clone(), _guard: guard, _locks: locks }))
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
    let Some(best) = ranked.first() else { return Ok(None) };
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
        fail!("cloning {} to {}: {}", source.display(), staging.display(), std::io::Error::last_os_error());
    }
    Ok(())
}

fn is_empty_dir(dir: &Path) -> Result<bool> {
    Ok(std::fs::read_dir(dir).context(|| format!("listing {}", dir.display()))?.next().is_none())
}

/// The blob git has for Cargo.lock at <rev> in <dir>, or "" when it has none.
fn lock_blob(dir: &Path, rev: &str) -> String {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "-q", &format!("{rev}:Cargo.lock")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Every other checkout of <checkout>'s repo whose `target` could be cloned
/// into a folder capped at <cap_kib>, unranked.
fn candidates(tool: &Tool, checkout: &Checkout, cap_kib: u64) -> Result<Vec<Candidate>> {
    let dest_root = checkout.build_root(&tool.places);
    let wanted = lock_blob(&checkout.top, "origin/main");
    let mut found: Vec<(Root, Checkout)> = Vec::new();
    for root in tool.roots()? {
        let Some(live) = root.live.clone() else { continue };
        if live.main != checkout.main || root.path == dest_root {
            continue;
        }
        if !target_of(&root.path).join("debug/.fingerprint").is_dir() {
            continue;
        }
        found.push((root, live));
    }
    let folders: Vec<PathBuf> = found.iter().map(|(root, _)| target_of(&root.path)).collect();
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
        candidates.push(Candidate {
            same_lock: lock_blob(&live.top, "HEAD") == wanted,
            root: root.path,
            finished,
        });
    }
    Ok(candidates)
}

/// Fills <checkout>'s host `target` from a warm checkout's when it is empty
/// (a new worktree, or a free one whose folder prune, a cap or the floor
/// deleted). Nothing to clone from is not a failure: the first build is cold,
/// as it always was.
pub fn fill(tool: &Tool, checkout: &Checkout) -> Result<()> {
    let Some(entry) = build_dirs(&checkout.top)?.into_iter().find(|entry| entry.path == "target" && entry.lane.is_none())
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
        note(format!("no warm host build of {} to clone; its first build is cold", checkout.repo()));
        return Ok(());
    };
    let source = target_of(&held.candidate.root);
    let staging = root.join(format!(".target.warm.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let cloned = clone_dir(&source, &staging).and_then(|()| {
        std::fs::rename(&staging, &dest).context(|| format!("moving the clone into {}", dest.display()))
    });
    if let Err(error) = cloned {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    drop(held);
    note(format!("cloned {} into {}", source.display(), dest.display()));
    // The source's incremental folders and the like: a side worktree's
    // builds read none of them.
    prune_root(tool, &root, checkout, Run::Real, 0, 0)
}

#[cfg(test)]
#[path = "warm_tests.rs"]
mod tests;
