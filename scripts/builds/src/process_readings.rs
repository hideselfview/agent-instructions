//! The process listings that each host's scans keep between scans, in the
//! file `process-readings/<host>.json` in builds' caches folder (~/Library/Caches/builds): for each
//! process, its pid, when it started and the CPU time it had used, at one
//! time. A scan (host_scan.rs) compares its listing with the newest reading
//! that is at least MIN_SECONDS old and at most MAX_SECONDS old, and a
//! process's CPU share is the CPU time it used since that reading over the
//! seconds between them. A process that the reading lacks, or that started
//! at another time (a new process on a reused pid), has no share yet. A host
//! gets a reading at most once a minute, and readings over MAX_SECONDS old
//! go.
//!
//! Health and the dashboard both write a host's file (readings_file.rs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fail::Result;
use crate::stuck::Row;

/// A reading younger than this says too little about what a process used.
pub const MIN_SECONDS: i64 = 10;
/// A reading older than this says nothing about what a process uses now.
pub const MAX_SECONDS: i64 = 15 * 60;
/// Two start times this close or closer are the same process's: `ps` prints
/// whole seconds of age, and each reading rounds them.
const START_TOLERANCE: i64 = 2;

/// One process in a reading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proc {
    pub pid: i32,
    /// Seconds since 1970 at which the process started.
    pub start: i64,
    /// CPU seconds it had used.
    pub cpu: f64,
}

/// One host's processes at one time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    /// Seconds since 1970, by the host's clock.
    pub at: i64,
    pub procs: Vec<Proc>,
}

impl Reading {
    /// The reading of <rows>, which a host listed at <at>.
    pub fn of(at: i64, rows: &[Row]) -> Reading {
        Reading {
            at,
            procs: rows
                .iter()
                .map(|row| Proc {
                    pid: row.pid,
                    start: at - row.age_seconds,
                    cpu: row.cpu_seconds,
                })
                .collect(),
        }
    }
}

/// The file of <host>'s readings in builds' caches folder under <home>.
fn file(home: &Path, host: &str) -> PathBuf {
    home.join("Library/Caches/builds/process-readings")
        .join(format!("{host}.json"))
}

/// The readings kept for <host>; none when there is no file yet.
pub fn read(home: &Path, host: &str) -> Result<Vec<Reading>> {
    crate::readings_file::read(&file(home, host))
}

/// Writes <readings> as <host>'s record, through a staged file of this
/// write's own.
pub fn write(home: &Path, host: &str, readings: &[Reading]) -> Result<()> {
    crate::readings_file::write(&file(home, host), readings)
}

/// Adds <reading> to <readings> when the newest is a minute old or more, and
/// drops those older than MAX_SECONDS. Returns whether it added it.
pub fn record(readings: &mut Vec<Reading>, reading: Reading) -> bool {
    let now = reading.at;
    crate::readings_file::record(readings, reading, now, |r| r.at, |_| true, MAX_SECONDS)
}

/// The newest of <readings> that is between MIN_SECONDS and MAX_SECONDS
/// before <now>.
fn compared(readings: &[Reading], now: i64) -> Option<&Reading> {
    readings
        .iter()
        .filter(|r| (MIN_SECONDS..=MAX_SECONDS).contains(&(now - r.at)))
        .max_by_key(|r| r.at)
}

/// <rows>, listed at <now>, with each CPU share set to what the process used
/// since the reading <compared> picks from <readings>, as a percentage of a
/// core, or to None when that reading has no such process.
pub fn measure(mut rows: Vec<Row>, now: i64, readings: &[Reading]) -> Vec<Row> {
    let previous = compared(readings, now);
    let before: HashMap<i32, &Proc> = previous
        .map(|reading| reading.procs.iter().map(|p| (p.pid, p)).collect())
        .unwrap_or_default();
    for row in &mut rows {
        row.cpu_percent = previous.and_then(|reading| {
            let earlier = before.get(&row.pid)?;
            let started = now - row.age_seconds;
            let used = row.cpu_seconds - earlier.cpu;
            ((started - earlier.start).abs() <= START_TOLERANCE && used >= 0.0)
                .then(|| used / (now - reading.at) as f64 * 100.0)
        });
    }
    rows
}

#[cfg(test)]
#[path = "process_readings_tests.rs"]
mod tests;
