//! The readings of each host's sccache server that `builds health` and the
//! dashboard keep between runs, in the file `sccache-host-readings.json` in
//! builds' caches folder: a server's counts at one time. Health compares
//! this Mac's count of finished compiles with a reading at least
//! READING_MINUTES old, to tell whether the server finished any while
//! clients wait (health.rs), and each host's Rust hits and misses with one,
//! for the share its cache answered lately (sccache_queue.rs). A host gets a
//! reading at most once a minute, and readings over an hour old go.
//!
//! Health and the dashboard both write the file (readings_file.rs).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fail::Result;

/// Readings this old or older say what a server did since.
pub const READING_MINUTES: i64 = 5;

/// One server's counts at one time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    /// The host's name; `local` for this Mac.
    pub host: String,
    /// The server's pid: a server that started again counts from zero.
    pub server: i32,
    /// Seconds since 1970.
    pub at: i64,
    /// Compile requests the server executed.
    pub executed: u64,
    /// Rust compiles the cache answered, and those it ran.
    pub rust_hits: u64,
    pub rust_misses: u64,
}

impl Reading {
    /// The reading of <stats>, which server <server> on <host> gave at <at>.
    pub fn of(host: &str, server: i32, at: i64, stats: &crate::sccache::CacheStats) -> Reading {
        Reading {
            host: host.to_string(),
            server,
            at,
            executed: stats.executed,
            rust_hits: stats.rust_hits,
            rust_misses: stats.rust_misses,
        }
    }
}

/// The file of readings in builds' caches folder under <home>.
fn file(home: &Path) -> PathBuf {
    home.join("Library/Caches/builds/sccache-host-readings.json")
}

/// The readings kept under <home>; none when there is no file yet.
pub fn read(home: &Path) -> Result<Vec<Reading>> {
    crate::readings_file::read(&file(home))
}

/// Writes <readings> as the record under <home>, through a staged file of
/// this write's own.
pub fn write(home: &Path, readings: &[Reading]) -> Result<()> {
    crate::readings_file::write(&file(home), readings)
}

/// Adds <reading> to <readings> when the newest of its host is a minute old
/// or more, and keeps the last hour. Returns whether it added it.
pub fn record(readings: &mut Vec<Reading>, reading: Reading) -> bool {
    let (now, host) = (reading.at, reading.host.clone());
    crate::readings_file::record(
        readings,
        reading,
        now,
        |r| r.at,
        |r| r.host == host,
        crate::readings_file::KEEP_SECONDS,
        3600,
    )
}

/// The newest of <readings> of server <server> on <host> at least
/// READING_MINUTES before <now>.
pub fn compared<'a>(
    readings: &'a [Reading],
    host: &str,
    server: i32,
    now: i64,
) -> Option<&'a Reading> {
    readings
        .iter()
        .filter(|r| r.host == host && r.server == server && now - r.at >= READING_MINUTES * 60)
        .max_by_key(|r| r.at)
}

#[cfg(test)]
#[path = "sccache_readings_tests.rs"]
mod tests;
