//! How well each host's compile cache served the Rust builds over the last
//! week. Each host's sccache server counts the Rust compiles its cache
//! answered (hits) and those it ran (misses) since the server started, so
//! this module keeps a sample of those counts every `EVERY_SECONDS`, for a
//! week, in the file `sccache-host-history.json` in builds' caches folder
//! (`builds health` and the dashboard daemon both add samples, as they do to
//! the readings of sccache_readings.rs, which cover only an hour). The rate
//! over the last hour, day or week is the growth of the counts across the
//! samples in it. A server that started again counts from zero: the growth
//! across its start is the new server's counts.
//!
//! `builds host cache` prints the rates, `builds health` adds them to each
//! host's compile cache line, and the dashboard draws them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fail::Result;
use crate::text::local_minute;

/// A host gets a sample when its newest is this old or older.
pub const EVERY_SECONDS: i64 = 300;
pub const HOUR: i64 = 3600;
pub const DAY: i64 = 24 * HOUR;
pub const WEEK: i64 = 7 * DAY;
/// Samples this old go: a week, and an hour more so that the sample at the
/// start of a week's window is still there.
pub const KEEP_SECONDS: i64 = WEEK + HOUR;
/// The windows the rates cover, by name.
pub const WINDOWS: [(&str, i64); 3] = [("hour", HOUR), ("day", DAY), ("week", WEEK)];

/// One server's counts at one time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// The host's name; `local` for this Mac.
    pub host: String,
    /// The server's pid: a server that started again counts from zero.
    pub server: i32,
    /// Seconds since 1970.
    pub at: i64,
    /// Rust compiles the cache answered, and those it ran, since the server
    /// started.
    pub hits: u64,
    pub misses: u64,
}

fn file(home: &Path) -> PathBuf {
    home.join("Library/Caches/builds/sccache-host-history.json")
}

/// The samples kept under <home>; none when there is no file yet.
pub fn read(home: &Path) -> Result<Vec<Sample>> {
    crate::readings_file::read(&file(home))
}

/// Writes <samples> as the record under <home>.
pub fn write(home: &Path, samples: &[Sample]) -> Result<()> {
    crate::readings_file::write(&file(home), samples)
}

/// Adds <sample> to <samples> when the newest of its host is
/// `EVERY_SECONDS` old or more, and drops those over `KEEP_SECONDS` old.
/// Returns whether it added it.
pub fn record(samples: &mut Vec<Sample>, sample: Sample) -> bool {
    if samples
        .iter()
        .any(|s| s.host == sample.host && sample.at - s.at < EVERY_SECONDS)
    {
        return false;
    }
    let now = sample.at;
    samples.push(sample);
    samples.retain(|s| now - s.at <= KEEP_SECONDS);
    true
}

/// The Rust compiles a host's cache answered and ran since <since>, which is
/// when the window starts, or later when the history starts later.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Rate {
    pub since: i64,
    pub hits: u64,
    pub misses: u64,
}

/// The rate of <host> over the <window> seconds before <now>: the growth of
/// its counts from the newest sample at or before the window's start (the
/// oldest sample in the window when none is) to the newest. None when fewer
/// than two samples are in reach.
pub fn rate(samples: &[Sample], host: &str, now: i64, window: i64) -> Option<Rate> {
    let mut of_host: Vec<&Sample> = samples.iter().filter(|s| s.host == host).collect();
    of_host.sort_by_key(|s| s.at);
    let start = now - window;
    let first = of_host
        .iter()
        .rposition(|s| s.at <= start)
        .unwrap_or_else(|| of_host.iter().position(|s| s.at > start).unwrap_or(0));
    let chain: Vec<&&Sample> = of_host[first..].iter().filter(|s| s.at <= now).collect();
    if chain.len() < 2 {
        return None;
    }
    let (mut hits, mut misses) = (0, 0);
    for pair in chain.windows(2) {
        let (before, after) = (pair[0], pair[1]);
        let grown = (
            after.hits.checked_sub(before.hits),
            after.misses.checked_sub(before.misses),
        );
        match grown {
            (Some(h), Some(m)) if before.server == after.server => {
                hits += h;
                misses += m;
            }
            _ => {
                hits += after.hits;
                misses += after.misses;
            }
        }
    }
    Some(Rate {
        since: chain[0].at,
        hits,
        misses,
    })
}

/// `90% of 100`, with `(since 2026-11-21 01:40)` when the history covers less
/// than the <window> seconds before <now>.
fn rate_text(rate: &Rate, now: i64, window: i64) -> String {
    let all = rate.hits + rate.misses;
    let share = if all == 0 {
        "no compile".to_string()
    } else {
        format!("{:.0}% of {all}", rate.hits as f64 * 100.0 / all as f64)
    };
    if rate.since > now - window {
        format!("{share} (since {})", local_minute(rate.since))
    } else {
        share
    }
}

/// The line that says each window's rate of <host>, as of <now>.
pub fn rates_text(samples: &[Sample], host: &str, now: i64) -> String {
    let rates: Vec<String> = WINDOWS
        .iter()
        .filter_map(|(name, window)| {
            let rate = rate(samples, host, now, *window)?;
            Some(format!("{name} {}", rate_text(&rate, now, *window)))
        })
        .collect();
    if rates.is_empty() {
        "hit rate of Rust compiles: no history yet".to_string()
    } else {
        format!("hit rate of Rust compiles: {}", rates.join(", "))
    }
}

/// One host's rates for the dashboard: each window's name and rate, None
/// where the history has too few samples.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HostRates {
    pub host: String,
    pub windows: Vec<(String, Option<Rate>)>,
}

/// The rates of every host that has a sample in <samples>, as of <now>, in
/// the order the hosts first appear.
pub fn host_rates(samples: &[Sample], now: i64) -> Vec<HostRates> {
    let mut hosts: Vec<&str> = Vec::new();
    for sample in samples {
        if !hosts.contains(&sample.host.as_str()) {
            hosts.push(&sample.host);
        }
    }
    hosts
        .into_iter()
        .map(|host| HostRates {
            host: host.to_string(),
            windows: WINDOWS
                .iter()
                .map(|(name, window)| (name.to_string(), rate(samples, host, now, *window)))
                .collect(),
        })
        .collect()
}

/// `builds host cache`: prints each host's hit rates, from the samples that
/// health and the dashboard daemon kept.
pub fn cmd_cache(tool: &crate::tool::Tool) -> Result<()> {
    let samples = read(&tool.places.home)?;
    let now = crate::app_timing::now();
    for host in crate::host::all(&tool.places)? {
        println!(
            "compile cache on {}: {}",
            crate::host::place(&host.name),
            rates_text(&samples, &host.name, now)
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "sccache_history_tests.rs"]
mod tests;
