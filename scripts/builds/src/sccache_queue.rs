//! Each host's sccache server as its scan shows it (host_scan.rs): the
//! compiles it runs (its child processes), the clients that wait for one
//! (the other processes named sccache that name a compiler), how full its
//! cache is, and the share of Rust compiles its cache answered since a
//! reading at least `sccache_readings::READING_MINUTES` old. `builds health`
//! prints a line for
//! each host, and lists under "needs action" a host whose waiting clients
//! number more than WAITING_PER_CORE times its cores: it has more compile
//! work than it can run, so new agents only wait longer.
//!
//! The readings come from the record health keeps of each server
//! (sccache_readings.rs), so the share covers the last few minutes, not the
//! server's whole life. A server that started again since a reading (another
//! pid, or counts that went down) is compared with nothing, and its share
//! covers its life.

use std::path::Path;

use crate::fail::Result;
use crate::health::Advice;
use crate::host::{Host, place};
use crate::host_scan::{Scan, arguments, program};
use crate::sccache::CacheStats;
use crate::sccache_history::Sample;
use crate::sccache_readings::Reading;
use crate::text::{human_bytes, local_minute};

/// More waiting clients than this many per core is more compile work than
/// the host can run.
const WAITING_PER_CORE: usize = 2;
/// A cache this share of its most or more is full: each new entry pushes an
/// old one out.
const FULL_SHARE: f64 = 0.99;

/// The Rust compiles a server's cache answered (hits) and those it ran
/// (misses), since <since> (seconds since 1970), or since the server started
/// when <since> is None.
#[derive(Clone, Debug, PartialEq)]
pub struct Rate {
    pub since: Option<i64>,
    pub hits: u64,
    pub misses: u64,
}

impl Rate {
    /// `50% of 824 Rust compiles since 01:40`.
    fn text(&self) -> String {
        let since = match self.since {
            Some(at) => format!("since {}", local_minute(at)),
            None => "since the server started".to_string(),
        };
        match self.hits + self.misses {
            0 => format!("no Rust compile {since}"),
            all => format!(
                "{:.0}% of {all} Rust compiles {since}",
                self.hits as f64 * 100.0 / all as f64
            ),
        }
    }
}

/// A host's server, as its scan shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub pid: i32,
    /// The compiles it runs: its child processes.
    pub running: usize,
    /// The clients: processes named sccache that name a compiler.
    pub clients: usize,
    pub stats: CacheStats,
    pub rate: Rate,
}

impl Server {
    /// The clients that wait for a compile: those beyond the compiles the
    /// server runs.
    pub fn waiting(&self) -> usize {
        self.clients.saturating_sub(self.running)
    }

    fn full(&self) -> bool {
        self.stats.cache_size as f64 >= self.stats.max_cache_size as f64 * FULL_SHARE
    }
}

/// A host and what its scan showed of its server: None when no server
/// listens, Err when the scan could not tell.
#[derive(Clone, Debug, PartialEq)]
pub struct Cache {
    pub host: Host,
    pub server: std::result::Result<Option<Server>, String>,
    /// The host's hit rate over the last hour, day and week
    /// (sccache_history.rs).
    pub history: String,
}

/// The rate of the server <pid> on <host> with <stats>, against the reading
/// of that server that `sccache_readings::compared` picks from <readings>.
fn rate(readings: &[Reading], host: &str, pid: i32, stats: &CacheStats, now: i64) -> Rate {
    let since_start = Rate {
        since: None,
        hits: stats.rust_hits,
        misses: stats.rust_misses,
    };
    let Some(before) = crate::sccache_readings::compared(readings, host, pid, now) else {
        return since_start;
    };
    match (
        stats.rust_hits.checked_sub(before.rust_hits),
        stats.rust_misses.checked_sub(before.rust_misses),
    ) {
        (Some(hits), Some(misses)) => Rate {
            since: Some(before.at),
            hits,
            misses,
        },
        _ => since_start,
    }
}

/// The server each of <scans> shows, with its rate against <readings>, which
/// gain a reading of each server.
fn judge_with(
    scans: &[(Host, std::result::Result<Scan, String>)],
    readings: &mut Vec<Reading>,
    history: &mut Vec<Sample>,
    now: i64,
) -> (Vec<Cache>, bool, bool) {
    let mut changed = false;
    let mut history_changed = false;
    let caches = scans
        .iter()
        .filter_map(|(host, scan)| {
            // stuck.rs lists a host whose scan failed.
            let scan = scan.as_ref().ok()?;
            let server = scan.sccache.clone().map(|server| {
                server.map(|server| {
                    let rate = rate(readings, &host.name, server.pid, &server.stats, now);
                    changed |= crate::sccache_readings::record(
                        readings,
                        Reading::of(&host.name, server.pid, now, &server.stats),
                    );
                    history_changed |= crate::sccache_history::record(
                        history,
                        Sample {
                            host: host.name.clone(),
                            server: server.pid,
                            at: now,
                            hits: server.stats.rust_hits,
                            misses: server.stats.rust_misses,
                        },
                    );
                    Server {
                        pid: server.pid,
                        running: scan
                            .rows
                            .iter()
                            .filter(|row| row.ppid == server.pid)
                            .count(),
                        clients: scan
                            .rows
                            .iter()
                            .filter(|row| {
                                row.pid != server.pid
                                    && program(row) == "sccache"
                                    && !arguments(row).is_empty()
                            })
                            .count(),
                        stats: server.stats,
                        rate,
                    }
                })
            });
            Some(Cache {
                host: host.clone(),
                server,
                history: crate::sccache_history::rates_text(history, &host.name, now),
            })
        })
        .collect();
    (caches, changed, history_changed)
}

/// Each host's server in <scans>, with the share of Rust compiles its cache
/// answered since a reading in the record kept under <home>
/// (sccache_readings.rs), to which it adds a reading of each server, and
/// each host's rates over the last week (sccache_history.rs), to which it
/// adds a sample of each server.
pub fn judge(
    home: &Path,
    scans: &[(Host, std::result::Result<Scan, String>)],
    now: i64,
) -> Result<Vec<Cache>> {
    let mut readings = crate::sccache_readings::read(home)?;
    let mut history = crate::sccache_history::read(home)?;
    let (caches, changed, history_changed) = judge_with(scans, &mut readings, &mut history, now);
    if changed {
        crate::sccache_readings::write(home, &readings)?;
    }
    if history_changed {
        crate::sccache_history::write(home, &history)?;
    }
    Ok(caches)
}

/// The line `builds health` prints for each of <caches>.
pub fn lines(caches: &[Cache]) -> Vec<String> {
    caches
        .iter()
        .map(|cache| {
            let place = place(&cache.host.name);
            match &cache.server {
                Err(error) => format!("compile cache on {place}: not read: {error}"),
                Ok(None) => format!("compile cache on {place}: no sccache server listens"),
                Ok(Some(server)) => format!(
                    "compile cache on {place} (sccache pid {}): {} compiles run, {} clients wait; it answered {}; cache {} of {}; {}",
                    server.pid,
                    server.running,
                    server.waiting(),
                    server.rate.text(),
                    human_bytes(server.stats.cache_size),
                    human_bytes(server.stats.max_cache_size),
                    cache.history,
                ),
            }
        })
        .collect()
}

/// The lines under "needs action": each host whose server has more than
/// WAITING_PER_CORE waiting clients per core.
pub fn findings(caches: &[Cache]) -> Vec<Advice> {
    caches
        .iter()
        .filter_map(|cache| {
            let server = cache.server.as_ref().ok()?.as_ref()?;
            let cores = cache.host.cores.max(1) as usize;
            if server.waiting() <= WAITING_PER_CORE * cores {
                return None;
            }
            let place = place(&cache.host.name);
            let full = if server.full() {
                format!(
                    "; its cache is full ({} of {}), so each new entry pushes an old one out",
                    human_bytes(server.stats.cache_size),
                    human_bytes(server.stats.max_cache_size)
                )
            } else {
                String::new()
            };
            Some(Advice::said(format!(
                "sccache: {place}'s server has {} clients waiting while it runs {} compiles, more than {WAITING_PER_CORE} for each of the host's {cores} cores, so the host has more compile work than it can run; its cache answered {}{full}; start nothing new for {place}",
                server.waiting(),
                server.running,
                server.rate.text(),
            )))
        })
        .collect()
}

#[cfg(test)]
#[path = "sccache_queue_tests.rs"]
mod tests;
