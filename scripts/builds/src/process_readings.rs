//! The process listings that each host's scans keep between scans, in the
//! file `process-readings/<host>.json` in builds' caches folder (~/Library/Caches/builds): for each
//! process, its pid, when it started and the CPU time it had used, at one
//! time. A scan (host_scan.rs) compares its listing with the newest reading
//! that is at least MIN_SECONDS old and at most MAX_SECONDS old, and a
//! process's CPU share is the CPU time it used since that reading over the
//! seconds between them. Over the span of SPAN_SECONDS, the lowest share in
//! any interval between readings is the process's `Row::sustained`. A process
//! that the reading lacks, or that started
//! at another time (a new process on a reused pid), has no share yet. A host
//! gets a reading at most once a minute, and readings over MAX_SECONDS old
//! go.
//!
//! Health and the dashboard both write a host's file (readings_file.rs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fail::Result;
use crate::stuck::{Row, Sustained};

/// A reading younger than this says too little about what a process used.
pub const MIN_SECONDS: i64 = 10;
/// A reading older than this says nothing about what a process uses now.
pub const MAX_SECONDS: i64 = 15 * 60;
/// How long a process must keep using CPU before it counts as busy for a
/// span: its share is over the threshold in every interval between the
/// readings of this many seconds, ending now.
pub const SPAN_SECONDS: i64 = 10 * 60;
/// Two start times this close or closer are the same process's: `ps` prints
/// whole seconds of age, and each reading rounds them.
const START_TOLERANCE: u64 = 2;

/// Whether two readings put a PID's start at the same time, within the
/// precision of ps's elapsed-time field.
pub fn same_start(earlier: i64, later: i64) -> bool {
    earlier.abs_diff(later) <= START_TOLERANCE
}

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
    crate::readings_file::record(
        readings,
        reading,
        now,
        |r| r.at,
        |_| true,
        crate::readings_file::KEEP_SECONDS,
        MAX_SECONDS,
    )
}

/// The newest of <readings> that is between <least> and MAX_SECONDS before
/// <now>.
fn newest_since(readings: &[Reading], now: i64, least: i64) -> Option<&Reading> {
    readings
        .iter()
        .filter(|r| (least..=MAX_SECONDS).contains(&(now - r.at)))
        .max_by_key(|r| r.at)
}

/// What <later> used since <earlier>, as a percentage of a core over <seconds>,
/// or None when they are not one process or <later> used less CPU time.
fn interval_share(earlier: &Proc, later: &Proc, seconds: i64) -> Option<f64> {
    let used = later.cpu - earlier.cpu;
    (same_start(earlier.start, later.start) && used >= 0.0).then(|| used / seconds as f64 * 100.0)
}

/// The lowest share that process <current> had in any interval between the
/// readings of <readings> from the newest one at least SPAN_SECONDS old (and
/// at most MAX_SECONDS) up to <now>, with the seconds that span covers. None
/// when no reading is old enough, or a reading in the span lacks the process.
fn sustained(readings: &[Reading], now: i64, current: &Proc) -> Option<Sustained> {
    let first = newest_since(readings, now, SPAN_SECONDS)?;
    let mut span: Vec<&Reading> = readings
        .iter()
        .filter(|r| r.at >= first.at && now - r.at >= MIN_SECONDS)
        .collect();
    span.sort_by_key(|r| r.at);
    let mut earlier: Option<(&Proc, i64)> = None;
    let mut lowest: Option<f64> = None;
    for reading in span {
        let proc = reading.procs.iter().find(|p| p.pid == current.pid)?;
        if let Some((before, at)) = earlier {
            let share = interval_share(before, proc, reading.at - at)?;
            lowest = Some(lowest.map_or(share, |l| l.min(share)));
        }
        earlier = Some((proc, reading.at));
    }
    let (before, at) = earlier?;
    let share = interval_share(before, current, now - at)?;
    Some(Sustained {
        percent: lowest.map_or(share, |l| l.min(share)),
        seconds: now - first.at,
    })
}

/// <rows>, listed at <now>, with each CPU share set to what the process used
/// since the reading `newest_since` picks from <readings>, as a percentage of a
/// core, or to None when that reading has no such process; and each
/// `sustained` set by `sustained` over the readings.
pub fn measure(mut rows: Vec<Row>, now: i64, readings: &[Reading]) -> Vec<Row> {
    let previous = newest_since(readings, now, MIN_SECONDS);
    let before: HashMap<i32, &Proc> = previous
        .map(|reading| reading.procs.iter().map(|p| (p.pid, p)).collect())
        .unwrap_or_default();
    for row in &mut rows {
        let current = Proc {
            pid: row.pid,
            start: now - row.age_seconds,
            cpu: row.cpu_seconds,
        };
        row.cpu_percent = previous
            .and_then(|reading| interval_share(before.get(&row.pid)?, &current, now - reading.at));
        row.sustained = sustained(readings, now, &current);
    }
    rows
}

#[cfg(test)]
#[path = "process_readings_tests.rs"]
mod tests;
