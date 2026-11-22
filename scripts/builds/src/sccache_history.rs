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

use serde::Serialize;

use crate::fail::Result;
use crate::sccache_queue::Rate;
use crate::sccache_readings::Reading;
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

fn file(home: &Path) -> PathBuf {
    home.join("Library/Caches/builds/sccache-host-history.json")
}

/// The samples kept under <home>; none when there is no file yet.
pub fn read(home: &Path) -> Result<Vec<Reading>> {
    crate::readings_file::read(&file(home))
}

/// Writes <samples> as the record under <home>.
pub fn write(home: &Path, samples: &[Reading]) -> Result<()> {
    crate::readings_file::write(&file(home), samples)
}

/// Adds <sample> to <samples> when the newest of its host is
/// `EVERY_SECONDS` old or more, and drops those over `KEEP_SECONDS` old.
/// Returns whether it added it.
pub fn record(samples: &mut Vec<Reading>, sample: Reading) -> bool {
    let (now, host) = (sample.at, sample.host.clone());
    crate::readings_file::record(
        samples,
        sample,
        now,
        |r| r.at,
        |r| r.host == host,
        EVERY_SECONDS,
        KEEP_SECONDS,
    )
}

/// The rate of <host> over the <window> seconds before <now>, which starts
/// when the window does or when the history does, if later: the growth of
/// its counts from the newest sample at or before the window's start (the
/// oldest sample in the window when none is) to the newest. None when fewer
/// than two samples are in reach.
pub fn rate(samples: &[Reading], host: &str, now: i64, window: i64) -> Option<Rate> {
    let mut of_host: Vec<&Reading> = samples.iter().filter(|s| s.host == host).collect();
    of_host.sort_by_key(|s| s.at);
    let start = now - window;
    let first = of_host
        .iter()
        .rposition(|s| s.at <= start)
        .unwrap_or_else(|| of_host.iter().position(|s| s.at > start).unwrap_or(0));
    let chain: Vec<&&Reading> = of_host[first..].iter().filter(|s| s.at <= now).collect();
    if chain.len() < 2 {
        return None;
    }
    let (mut hits, mut misses) = (0, 0);
    for pair in chain.windows(2) {
        let (before, after) = (pair[0], pair[1]);
        let grown = (
            after.rust_hits.checked_sub(before.rust_hits),
            after.rust_misses.checked_sub(before.rust_misses),
        );
        match grown {
            (Some(h), Some(m)) if before.server == after.server => {
                hits += h;
                misses += m;
            }
            _ => {
                hits += after.rust_hits;
                misses += after.rust_misses;
            }
        }
    }
    Some(Rate {
        since: Some(chain[0].at),
        hits,
        misses,
    })
}

/// `90% of 100`, with `(since 2026-11-21 01:40)` when the history covers less
/// than the <window> seconds before <now>.
fn rate_text(rate: &Rate, now: i64, window: i64) -> String {
    let share = rate.share().unwrap_or_else(|| "no compile".to_string());
    match rate.since {
        Some(since) if since > now - window => format!("{share} (since {})", local_minute(since)),
        _ => share,
    }
}

/// The line that says each window's rate of <host>, as of <now>.
pub fn rates_text(samples: &[Reading], host: &str, now: i64) -> String {
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
pub fn host_rates(samples: &[Reading], now: i64) -> Vec<HostRates> {
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
