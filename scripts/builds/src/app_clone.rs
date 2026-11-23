//! Choosing the checkout that a new app checkout starts from.
//!
//! A new app checkout of a lane starts from a copy-on-write clone of the
//! lane's warmest checkout on the same host (warm.rs `clone_lane`), so it
//! takes almost no new disk and its first build compiles little. The warmest
//! checkout is the one whose lock a build let go of most recently, since a
//! checkout that no build used holds nothing built. The clone is taken while
//! the caller holds the source's lock, so no app build runs there, as `builds
//! new` holds the guard of the checkout it clones.
//!
//! A lane's checkouts are built in turn, so when builds adds a checkout
//! because builds wait, every other checkout of the lane is building. The
//! caller then waits for the warmest source's lock (lane_size.rs
//! `wait_checkout`), which its holder lets go when its steps end, within their
//! limits (`app_config::DEFAULT_STEP_LIMIT`).

use crate::app_remote::host_of;
use crate::fail::Result;
use crate::lane_size::{CheckoutLock, RepoLane, Waited, last_use, lock_checkout, wait_checkout};
use crate::queue::Queue;
use crate::text::note;

/// The entries of <used>, `(entry, when a build last let go of its lock)`,
/// that a new checkout may start from, warmest first: the ones some build
/// used, the most recently used first, and by name among equals. An entry no
/// build used holds nothing built.
pub fn rank_sources(used: &[(String, Option<i64>)]) -> Vec<String> {
    let mut warm: Vec<(i64, &str)> = used
        .iter()
        .filter_map(|(entry, last)| last.map(|last| (last, entry.as_str())))
        .collect();
    warm.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    warm.into_iter()
        .map(|(_, entry)| entry.to_string())
        .collect()
}

/// A source checkout, which this value keeps locked until it is dropped.
pub struct Source {
    pub entry: String,
    pub lock: CheckoutLock,
}

/// The lane's checkouts on <host>, other than <new_entry> when there is one,
/// warmest first.
fn ranked(at: RepoLane, host: &str, new_entry: Option<&str>) -> Result<Vec<String>> {
    let mut used = Vec::new();
    for entry in at.places.app_checkouts_of(at.repo, at.lane)? {
        if Some(entry.as_str()) != new_entry && host_of(&entry) == host {
            let last = last_use(at, &entry)?;
            used.push((entry, last));
        }
    }
    Ok(rank_sources(&used))
}

/// Does <host> have a checkout of <at>'s lane that a new checkout there starts
/// from? The caller holds the lane's queue guard or does not need the answer
/// to stay true.
pub fn has_source(at: RepoLane, host: &str) -> Result<bool> {
    Ok(!ranked(at, host, None)?.is_empty())
}

/// Locks the warmest checkout of <at>'s lane on <host>, other than
/// <new_entry>, that no build holds; when every one is held, waits for the
/// warmest one's lock. None when the host has no such checkout, or when a
/// takeover stopped this build during the wait. The lane's set is read and
/// the locks tried under the lane's queue guard, as every take of a lock is
/// (lane_size.rs); the wait holds no guard.
pub fn take_source(at: RepoLane, host: &str, new_entry: &str) -> Result<Option<Source>> {
    loop {
        let best = {
            let _guard = Queue::of(at.places, at.repo, at.lane).guard()?;
            let candidates = ranked(at, host, Some(new_entry))?;
            for entry in &candidates {
                if let Some(lock) = lock_checkout(at.places, at.repo, at.lane, entry)? {
                    return Ok(Some(Source {
                        entry: entry.clone(),
                        lock,
                    }));
                }
            }
            let Some(best) = candidates.into_iter().next() else {
                return Ok(None);
            };
            best
        };
        note(format!(
            "every warm {} checkout of {} on {host} is building; {new_entry} waits for {best}, which it starts from",
            at.lane.word(),
            at.repo
        ));
        match wait_checkout(at.places, at.repo, at.lane, &best)? {
            Waited::Taken(lock) => {
                return Ok(Some(Source { entry: best, lock }));
            }
            Waited::Stopped => {
                note(format!(
                    "a takeover ended the wait for {best}, so {new_entry} starts cold"
                ));
                return Ok(None);
            }
            // The checkout left the lane while this waited: choose again.
            Waited::Gone => {}
        }
    }
}

#[cfg(test)]
#[path = "app_clone_tests.rs"]
mod tests;
