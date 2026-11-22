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
//! caller then waits for the best source's lock, up to `SOURCE_WAIT`, and
//! starts cold when none comes free in that time.

use std::time::{Duration, Instant};

use crate::app_remote::split;
use crate::fail;
use crate::fail::Result;
use crate::host::LOCAL;
use crate::lane_size::{CheckoutLock, RepoLane, last_use, lock_checkout};
use crate::queue::Queue;
use crate::text::note;

/// How long a new checkout waits for a source's lock. An app build's steps
/// end within their limits (`app_config::DEFAULT_STEP_LIMIT`), and most take
/// well under ten minutes warm, so a source frees up within about that time;
/// past it, the cold first build is the shorter wait.
pub const SOURCE_WAIT: Duration = Duration::from_secs(10 * 60);

/// How often the wait looks at the sources' locks again.
const POLL: Duration = Duration::from_secs(1);

/// The host that <entry> names: the host before the colon, or this Mac.
fn host_of(entry: &str) -> &str {
    split(entry).map_or(LOCAL, |(host, _)| host)
}

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
    _lock: CheckoutLock,
}

/// The lane's checkouts on <host> other than <new_entry>, warmest first.
fn ranked(at: RepoLane, host: &str, new_entry: &str) -> Result<Vec<String>> {
    let mut used = Vec::new();
    for entry in at.places.app_checkouts_of(at.repo, at.lane)? {
        if entry != new_entry && host_of(&entry) == host {
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
    Ok(!ranked(at, host, "")?.is_empty())
}

/// Locks the warmest checkout of <at>'s lane on <host>, other than
/// <new_entry>, that no build holds, and waits up to <wait> for the warmest
/// one when every one is held. None when the host has no such checkout or none
/// came free in time. The lane's set is read and the locks tried under the
/// lane's queue guard, as every take of a lock is (lane_size.rs).
pub fn take_source(
    at: RepoLane,
    host: &str,
    new_entry: &str,
    wait: Duration,
) -> Result<Option<Source>> {
    let started = Instant::now();
    loop {
        let candidates = {
            let _guard = Queue::of(at.places, at.repo, at.lane).guard()?;
            let candidates = ranked(at, host, new_entry)?;
            for entry in &candidates {
                if let Some(lock) = lock_checkout(at.places, at.repo, at.lane, entry)? {
                    return Ok(Some(Source {
                        entry: entry.clone(),
                        _lock: lock,
                    }));
                }
            }
            candidates
        };
        let Some(best) = candidates.first() else {
            return Ok(None);
        };
        if started.elapsed() >= wait {
            note(format!(
                "every warm {} checkout of {} on {host} stayed in use for {} seconds, so {new_entry} starts cold",
                at.lane.word(),
                at.repo,
                wait.as_secs()
            ));
            return Ok(None);
        }
        if let Some(signal) = crate::lock::stop_signal() {
            fail!(
                "{} ended the wait for {best}, the checkout {new_entry} starts from",
                crate::lock::signal_name(signal)
            );
        }
        std::thread::sleep(POLL.min(wait));
    }
}

#[cfg(test)]
#[path = "app_clone_tests.rs"]
mod tests;
